use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once, OnceLock};

use rusqlite::{params, Connection};
use serde::Serialize;

/// One indexed screenshot. The PNG on disk stays the source of truth; this is
/// metadata only, so losing the database never loses an image.
#[derive(Debug, Clone, Serialize)]
pub struct Screenshot {
    pub path: String,
    pub created_at: i64,
    pub width: u32,
    pub height: u32,
    pub source: String,
}

/// One search hit: the screenshot plus mode-specific context.
///
/// `snippet` is set by keyword search only. It is an FTS5 excerpt with match
/// boundaries marked by \u{1f}…\u{1e} (control chars, never real OCR text),
/// so the frontend can highlight without trusting raw HTML from the index.
/// `score` is set by semantic search only: 0..1 similarity, higher is closer.
/// `score_z` is the same cosine standardised against this result set
/// (`(cos - mean) / std`): on a homogeneous screenshot corpus raw cosines pack
/// into 0.9..1.0 and carry no information, while +2.8σ vs +0.3σ reads.
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub path: String,
    pub created_at: i64,
    pub width: u32,
    pub height: u32,
    pub source: String,
    pub snippet: Option<String>,
    pub score: Option<f32>,
    pub score_z: Option<f32>,
}

impl SearchHit {
    fn text(
        path: String,
        created_at: i64,
        width: i64,
        height: i64,
        source: String,
        snippet: Option<String>,
    ) -> Self {
        Self {
            path,
            created_at,
            width: width as u32,
            height: height as u32,
            source,
            snippet,
            score: None,
            score_z: None,
        }
    }

    fn visual(
        path: String,
        created_at: i64,
        width: i64,
        height: i64,
        source: String,
        distance: f64,
    ) -> Self {
        Self {
            path,
            created_at,
            width: width as u32,
            height: height as u32,
            source,
            snippet: None,
            score: Some(l2_to_cosine(distance) as f32),
            score_z: None,
        }
    }
}

/// Full per-screenshot record for the details view: everything the index
/// knows, including the OCR text itself (never returned by list/search).
#[derive(Debug, Clone, Serialize)]
pub struct ScreenshotDetail {
    pub path: String,
    pub created_at: i64,
    pub width: u32,
    pub height: u32,
    pub source: String,
    /// `None` = OCR never attempted; `Some("")` = done, no readable text.
    pub ocr_text: Option<String>,
    pub ocr_word_count: usize,
    /// True once OCR was attempted (even with zero words found).
    pub ocr_indexed: bool,
    pub embedded_at: Option<i64>,
    pub embed_model: Option<String>,
}

pub fn db_path() -> Result<PathBuf, String> {
    // Test hook: the import test must not touch the real index.
    if let Ok(custom) = std::env::var("XREENSHOTY_TEST_DB") {
        return Ok(PathBuf::from(custom));
    }
    let base = dirs::data_local_dir().ok_or_else(|| "No data dir".to_string())?;
    Ok(base.join("xreenshoty").join("screenshots.db"))
}

/// EmbeddingGemma 2 outputs 768-dim vectors. Anything that indexes an
/// embedding must agree with this or the `vec0` insert fails.
pub const EMBED_DIM: usize = 768;

/// Registers the sqlite-vec extension. Must happen before any connection is
/// opened; `sqlite3_auto_extension` is process-global, so once is enough.
///
/// The `transmute` bridges a mismatch in rusqlite's binding: SQLite itself
/// wants a zero-argument entry point (`void(*)(void)`, and `sqlite3_vec_init`
/// is declared exactly that way), but the Rust wrapper is typed as
/// `fn(Option<unsafe extern "C" fn()>) -> c_int` — it takes the entry point as
/// an argument. Rust cannot coerce a 0-arg fn into a 1-arg slot, and both sides
/// model the entry point as `fn()`, so the cast is sound.
fn register_vec_extension() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

/// Adds `column` unless it is already there. `ALTER TABLE ADD COLUMN` has no
/// `IF NOT EXISTS`, and this schema is applied against databases that already
/// exist from earlier runs.
fn add_column(conn: &Connection, column: &str, decl: &str) -> Result<(), String> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(screenshots)")
        .map_err(|e| format!("table_info failed: {e}"))?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| format!("table_info read failed: {e}"))?
        .any(|name| name.is_ok_and(|n| n == column));
    if exists {
        return Ok(());
    }
    conn.execute_batch(&format!(
        "ALTER TABLE screenshots ADD COLUMN {column} {decl}"
    ))
    .map_err(|e| format!("Add column {column} failed: {e}"))
}

/// Search tables: FTS5 over OCR text, and `vec0` over embeddings.
///
/// Deliberately best-effort. This is an index over files that already exist on
/// disk, so a broken extension must degrade search rather than make the app
/// unable to record a screenshot. Failures are logged, not raised.
fn ensure_search_schema(conn: &Connection) {
    if let Err(e) = conn.execute_batch(
        "
        CREATE VIRTUAL TABLE IF NOT EXISTS screenshots_fts USING fts5(
            ocr_text,
            content='screenshots',
            content_rowid='id'
        );

        CREATE TRIGGER IF NOT EXISTS screenshots_fts_ai AFTER INSERT ON screenshots
        WHEN new.ocr_text IS NOT NULL AND new.ocr_text != ''
        BEGIN
            INSERT INTO screenshots_fts(rowid, ocr_text) VALUES (new.id, new.ocr_text);
        END;

        CREATE TRIGGER IF NOT EXISTS screenshots_fts_ad AFTER DELETE ON screenshots
        BEGIN
            INSERT INTO screenshots_fts(screenshots_fts, rowid, ocr_text)
            SELECT 'delete', old.id, old.ocr_text
            WHERE old.ocr_text IS NOT NULL AND old.ocr_text != '';
        END;

        CREATE TRIGGER IF NOT EXISTS screenshots_fts_au AFTER UPDATE ON screenshots
        WHEN old.ocr_text IS NOT new.ocr_text
        BEGIN
            INSERT INTO screenshots_fts(screenshots_fts, rowid, ocr_text)
            SELECT 'delete', old.id, old.ocr_text
            WHERE old.ocr_text IS NOT NULL AND old.ocr_text != '';
            INSERT INTO screenshots_fts(rowid, ocr_text)
            SELECT new.id, new.ocr_text
            WHERE new.ocr_text IS NOT NULL AND new.ocr_text != '';
        END;
        ",
    ) {
        log::warn!("FTS5 schema unavailable, keyword search disabled: {e}");
        // Deliberately no early return: the two search tables are independent
        // features. Losing keyword search must not also take out semantic.
    }

    if let Err(e) = conn.execute_batch(&format!(
        // `distance_metric=cosine` is explicit on purpose: vec0 defaults to L2, which
        // is not a similarity and has to be converted back for display.
        "CREATE VIRTUAL TABLE IF NOT EXISTS screenshot_vec USING vec0(embedding float[{EMBED_DIM}] distance_metric=cosine);"
    )) {
        log::warn!("vec0 schema unavailable, semantic search disabled: {e}");
    }
}

/// Open the database and ensure the schema. Called once per path — every
/// caller goes through `conn()`, which caches the result.
fn open(path: &Path) -> Result<Connection, String> {
    register_vec_extension();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Create data dir failed: {e}"))?;
    }
    let conn = Connection::open(path).map_err(|e| format!("Open database failed: {e}"))?;

    // Metadata is load-bearing: a failure here must surface.
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS screenshots (
             id         INTEGER PRIMARY KEY,
             path       TEXT    NOT NULL UNIQUE,
             created_at INTEGER NOT NULL,
             width      INTEGER NOT NULL,
             height     INTEGER NOT NULL,
             source     TEXT    NOT NULL DEFAULT 'capture'
         );
         CREATE INDEX IF NOT EXISTS idx_screenshots_created
             ON screenshots (created_at DESC);",
    )
    .map_err(|e| format!("Schema setup failed: {e}"))?;

    for (column, decl) in [
        ("ocr_text", "TEXT"),
        ("ocr_boxes", "TEXT"),
        ("embedded_at", "INTEGER"),
        ("embed_model", "TEXT"),
    ] {
        add_column(&conn, column, decl)?;
    }

    ensure_search_schema(&conn);
    Ok(conn)
}

/// Cached connections, keyed by resolved path.
///
/// Reopening the file and re-running the schema DDL on every command was pure
/// overhead: `list_recent` alone fired four of these per history refresh. Keying
/// by path (rather than a single global connection) keeps the test DB override
/// isolated from the real index instead of aliasing them.
fn conn() -> Result<Arc<Mutex<Connection>>, String> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Connection>>>>> = OnceLock::new();

    let path = db_path()?;
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    if let Some(existing) = cache.get(&path) {
        return Ok(existing.clone());
    }
    let fresh = Arc::new(Mutex::new(open(&path)?));
    cache.insert(path, fresh.clone());
    Ok(fresh)
}

/// Run `f` with the cached connection. A poisoned lock is recovered rather than
/// propagated; the database is an index, and a panic elsewhere must not take it
/// offline.
pub fn with_conn<T>(f: impl FnOnce(&Connection) -> Result<T, String>) -> Result<T, String> {
    let handle = conn()?;
    let guard = handle.lock().unwrap_or_else(|e| e.into_inner());
    f(&guard)
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Index a saved file. Best effort: a database problem must never lose a
/// screenshot that is already on disk, so failures are logged, not fatal.
pub fn record(path: &Path, width: u32, height: u32, source: &str) {
    if let Err(e) = insert(path, now_millis(), width, height, source) {
        log::warn!("could not index {}: {e}", path.display());
    }
}

fn insert(path: &Path, created_at: i64, width: u32, height: u32, source: &str) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute(
            "INSERT INTO screenshots (path, created_at, width, height, source)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET
                 width  = excluded.width,
                 height = excluded.height",
            params![
                path.to_string_lossy(),
                created_at,
                width as i64,
                height as i64,
                source
            ],
        )
        .map_err(|e| format!("Insert failed: {e}"))?;
        Ok(())
    })
}

/// Width/height straight out of the PNG IHDR chunk.
///
/// PNG stores these as fixed big-endian u32 at bytes 16..24: 8-byte signature,
/// 4-byte chunk length, `"IHDR"`, then width and height. Decoding the image
/// just to learn its size costs orders of magnitude more, and `import_existing`
/// does this once per file in the save folder on every launch.
pub(crate) fn png_dimensions(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read;

    let mut f = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 24];
    f.read_exact(&mut header).ok()?;

    if &header[..8] != b"\x89PNG\r\n\x1a\n" || &header[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(header[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(header[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

/// Index PNGs already sitting in `dir`. Idempotent via the UNIQUE path column,
/// so running it every launch is safe and needs no bookkeeping flag.
/// Drop rows whose PNG has been deleted outside the app.
///
/// Files are the source of truth, not the DB (AGENTS.md): a row for a file that
/// no longer exists is a ghost. It occupies a `list_recent` slot, its thumbnail
/// fails so the card renders "…", and it still ranks in search.
///
/// Existence is checked per row rather than by "not in the folder we just
/// imported", so rows from a previously-configured save folder survive as long
/// as those files are still there.
///
/// FTS5 is cleaned by the `screenshots_fts_ad` trigger on the row delete, but
/// `vec0` has no triggers — its rows must be deleted explicitly or the vectors
/// outlive the screenshots and keep skewing KNN.
pub fn prune_missing() -> Result<usize, String> {
    let paths: Vec<(i64, String)> = with_conn(|conn| {
        let mut stmt = conn
            .prepare("SELECT id, path FROM screenshots")
            .map_err(|e| format!("Prepare failed: {e}"))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| format!("Query failed: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row read failed: {e}"))
    })?;

    let gone: Vec<i64> = paths
        .iter()
        .filter(|(_, path)| !std::path::Path::new(path).exists())
        .map(|(id, _)| *id)
        .collect();

    if gone.is_empty() {
        return Ok(0);
    }

    with_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Transaction failed: {e}"))?;
        for id in &gone {
            tx.execute("DELETE FROM screenshot_vec WHERE rowid = ?1", params![id])
                .map_err(|e| format!("Vector delete failed: {e}"))?;
            tx.execute("DELETE FROM screenshots WHERE id = ?1", params![id])
                .map_err(|e| format!("Row delete failed: {e}"))?;
        }
        tx.commit().map_err(|e| format!("Commit failed: {e}"))
    })?;

    log::info!("pruned {} row(s) whose file no longer exists", gone.len());
    Ok(gone.len())
}

#[tauri::command]
pub async fn import_existing(dir: Option<String>) -> Result<usize, String> {
    // Reconcile before adding: a PNG deleted outside the app otherwise keeps a
    // ghost row forever. Best-effort — a prune failure must not stop the import.
    let _ = prune_missing();
    let dir = crate::screenshot::resolve_dir(dir)?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => return Err(format!("Read save dir failed: {e}")),
    };

    let mut imported = 0usize;
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "INSERT INTO screenshots (path, created_at, width, height, source)
                 VALUES (?1, ?2, ?3, ?4, 'import')
                 ON CONFLICT(path) DO NOTHING",
            )
            .map_err(|e| format!("Prepare failed: {e}"))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("png") {
                continue;
            }
        // Cheap pre-check so we do not decode every PNG on disk every launch.
        let already = conn
            .query_row(
                "SELECT 1 FROM screenshots WHERE path = ?1",
                params![path.to_string_lossy()],
                |_| Ok(()),
            )
            .is_ok();
        if already {
            continue;
        }

        let Some((w, h)) = png_dimensions(&path) else {
            continue;
        };
        let created_at = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or_else(now_millis);

        stmt.execute(params![
            path.to_string_lossy(),
            created_at,
            w as i64,
            h as i64
        ])
        .map_err(|e| format!("Insert failed: {e}"))?;
            imported += 1;
        }
        Ok(())
    })?;

    log::info!("imported {imported} existing screenshots from {}", dir.display());
    Ok(imported)
}

#[tauri::command]
pub async fn list_recent(limit: Option<u32>) -> Result<Vec<Screenshot>, String> {
    let limit = limit.unwrap_or(60).clamp(1, 500) as i64;
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT path, created_at, width, height, source
                 FROM screenshots ORDER BY created_at DESC LIMIT ?1",
            )
            .map_err(|e| format!("Prepare failed: {e}"))?;

        let rows = stmt
            .query_map(params![limit], |row| {
                Ok(Screenshot {
                    path: row.get(0)?,
                    created_at: row.get(1)?,
                    width: row.get::<_, i64>(2)? as u32,
                    height: row.get::<_, i64>(3)? as u32,
                    source: row.get(4)?,
                })
            })
            .map_err(|e| format!("Query failed: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row read failed: {e}"))
    })
}

#[tauri::command]
pub async fn screenshot_count() -> Result<i64, String> {
    with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM screenshots", [], |row| row.get(0))
            .map_err(|e| format!("Count failed: {e}"))
    })
}

/// Serialises database tests.
///
/// `XREENSHOTY_TEST_DB` is process-global, so two DB tests running in parallel
/// clobber each other's path and read each other's rows. This is not
/// hypothetical: cargo runs tests on separate threads by default.
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod search_schema_tests {
    use super::*;

    fn test_db(name: &str) -> PathBuf {
        let db = std::env::temp_dir().join(name);
        for suffix in ["", "-wal", "-shm"] {
            std::fs::remove_file(format!("{}{suffix}", db.display())).ok();
        }
        std::env::set_var("XREENSHOTY_TEST_DB", &db);
        db
    }

    /// 5.1's whole promise: the columns exist, FTS5 mirrors `ocr_text` through
    /// its triggers, and `vec0` accepts a real 768-d vector and ranks it.
    #[test]
    fn search_schema_is_complete_and_live() {
        let _guard = super::test_lock();
        test_db("xreenshoty-search-schema.db");

        // Opening twice must be safe: schema setup runs on every new path.
        with_conn(|_| Ok(())).unwrap();
        with_conn(|_| Ok(())).unwrap();

        with_conn(|conn| {
            let cols: Vec<String> = conn
                .prepare("PRAGMA table_info(screenshots)")
                .unwrap()
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            for expected in ["ocr_text", "ocr_boxes", "embedded_at", "embed_model"] {
                assert!(cols.iter().any(|c| c == expected), "missing column {expected}");
            }

            conn.execute(
                "INSERT INTO screenshots (path, created_at, width, height, source, ocr_text, embed_model)
                 VALUES ('/tmp/a.png', 1, 10, 10, 'capture', 'segmentation fault in kernel', 'embeddinggemma-2')",
                [],
            ).map_err(|e| format!("insert: {e}"))?;

            // FTS5 must have mirrored the row via its AFTER INSERT trigger.
            let hits: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM screenshots_fts WHERE screenshots_fts MATCH 'kernel'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| format!("fts query: {e}"))?;
            assert_eq!(hits, 1, "FTS5 did not index the inserted ocr_text");

            // Updating the text must remove the stale term (UPDATE trigger).
            conn.execute(
                "UPDATE screenshots SET ocr_text = 'permission denied' WHERE path = '/tmp/a.png'",
                [],
            ).map_err(|e| format!("update: {e}"))?;
            let stale: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM screenshots_fts WHERE screenshots_fts MATCH 'kernel'",
                    [],
                    |r| r.get(0),
                )
                .map_err(|e| format!("fts re-query: {e}"))?;
            assert_eq!(stale, 0, "stale term survived the update trigger");

            // vec0: insert a real 768-d vector and find it by similarity.
            let vec: Vec<f32> = (0..EMBED_DIM).map(|i| i as f32 / EMBED_DIM as f32).collect();
            let mut bytes = Vec::with_capacity(vec.len() * 4);
            for v in &vec {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            let id: i64 = conn
                .query_row("SELECT id FROM screenshots WHERE path = '/tmp/a.png'", [], |r| r.get(0))
                .map_err(|e| format!("get id: {e}"))?;
            conn.execute(
                "INSERT INTO screenshot_vec(rowid, embedding) VALUES (?1, ?2)",
                rusqlite::params![id, bytes],
            ).map_err(|e| format!("vec insert: {e}"))?;

            let nearest: i64 = conn
                .query_row(
                    "SELECT rowid FROM screenshot_vec WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1",
                    rusqlite::params![bytes],
                    |r| r.get(0),
                )
                .map_err(|e| format!("vec match: {e}"))?;
            assert_eq!(nearest, id, "vector search returned the wrong row");

            Ok(())
        }).unwrap();

        for suffix in ["", "-wal", "-shm"] {
            std::fs::remove_file(format!(
                "{}{suffix}",
                std::env::temp_dir().join("xreenshoty-search-schema.db").display()
            )).ok();
        }
    }
}

/// A row the indexer still owes work on.
pub struct Pending {
    pub id: i64,
    pub path: String,
    pub needs_ocr: bool,
    pub needs_embedding: bool,
}

/// Rows that are not fully indexed yet. `limit` keeps each pass bounded.
///
/// `needs_ocr DESC` first: OCR needs no GPU, so rows that still owe text must
/// not sit behind rows the embedding server is unable to drain. Ordered by
/// `created_at` alone, a library with no model server kept re-returning the
/// same oldest rows — they were OCR'd on pass 1, but still counted as pending
/// because they needed embedding, so the queue head never advanced past them and
/// every later screenshot was never OCR'd at all. Within each group the
/// original backfill order is preserved.
pub fn pending(limit: u32) -> Result<Vec<Pending>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, path,
                        (ocr_boxes IS NULL) AS needs_ocr,
                        (embedded_at IS NULL) AS needs_embedding
                 FROM screenshots
                 WHERE needs_ocr OR needs_embedding
                 ORDER BY needs_ocr DESC, created_at ASC
                 LIMIT ?1",
            )
            .map_err(|e| format!("Prepare failed: {e}"))?;

        let rows = stmt
            .query_map(params![limit.clamp(1, 200) as i64], |row| {
                Ok(Pending {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    needs_ocr: row.get::<_, i64>(2)? != 0,
                    needs_embedding: row.get::<_, i64>(3)? != 0,
                })
            })
            .map_err(|e| format!("Query failed: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row read failed: {e}"))
    })
}

/// `needs_ocr` keys off `ocr_boxes`, not `ocr_text`: a screenshot with no
/// readable text legitimately yields an empty `ocr_text`, and treating that as
/// "not done" would re-OCR those files on every pass forever. `ocr_boxes` is
/// written (`[]`) even when nothing was found, so it records "attempted".
pub fn set_ocr(id: i64, text: &str, words_json: &str) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute(
            "UPDATE screenshots SET ocr_text = ?1, ocr_boxes = ?2 WHERE id = ?3",
            params![text, words_json, id],
        )
        .map_err(|e| format!("Set OCR failed: {e}"))?;
        Ok(())
    })
}

/// Stores the vector and records which model produced it, so a later model
/// change can be detected instead of silently mixing incompatible embeddings.
pub fn set_embedding(id: i64, bytes: &[u8], model: &str) -> Result<(), String> {
    let now = now_millis();
    with_conn(|conn| {
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Transaction failed: {e}"))?;
        // `vec0` does not implement UPSERT, so replace explicitly. Both
        // statements share the transaction, so a failure cannot leave the
        // vector and its timestamp disagreeing.
        tx.execute("DELETE FROM screenshot_vec WHERE rowid = ?1", params![id])
            .map_err(|e| format!("Vector delete failed: {e}"))?;
        tx.execute(
            "INSERT INTO screenshot_vec(rowid, embedding) VALUES (?1, ?2)",
            params![id, bytes],
        )
        .map_err(|e| format!("Vector write failed: {e}"))?;
        tx.execute(
            "UPDATE screenshots SET embedded_at = ?1, embed_model = ?2 WHERE id = ?3",
            params![now, model, id],
        )
        .map_err(|e| format!("Embedding stamp failed: {e}"))?;
        tx.commit().map_err(|e| format!("Commit failed: {e}"))
    })
}

/// Records that embedding was attempted and did not happen, so the indexer does
/// not re-stamp the same rows on every idle poll while the model is loading.
///
/// This deliberately leaves `embedded_at` NULL: the row stays *pending*, so it
/// is retried automatically once a server does come up. Marking it done here
/// would strand every screenshot captured before the model was installed —
/// they would never be embedded, even after the user starts one.
pub fn mark_embedding_skipped(id: i64) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute(
            "UPDATE screenshots SET embed_model = 'skipped'
             WHERE id = ?1 AND embedded_at IS NULL AND embed_model IS NOT 'skipped'",
            params![id],
        )
        .map_err(|e| format!("Skip stamp failed: {e}"))?;
        Ok(())
    })
}

/// Progress of the background indexer, for the UI.
#[derive(Serialize, Clone)]
pub struct IndexProgress {
    /// Screenshots with a stored vector.
    pub embedded: i64,
    /// Screenshots with OCR attempted (an empty result still counts).
    pub ocred: i64,
    pub total: i64,
}

pub fn index_progress() -> Result<IndexProgress, String> {
    with_conn(|conn| {
        let count = |sql: &str| -> Result<i64, String> {
            conn.query_row(sql, [], |r| r.get(0))
                .map_err(|e| format!("Count failed: {e}"))
        };
        Ok(IndexProgress {
            embedded: count("SELECT COUNT(*) FROM screenshots WHERE embedded_at IS NOT NULL")?,
            ocred: count("SELECT COUNT(*) FROM screenshots WHERE ocr_boxes IS NOT NULL")?,
            total: count("SELECT COUNT(*) FROM screenshots")?,
        })
    })
}

pub fn indexed_count() -> Result<(i64, i64), String> {
    with_conn(|conn| {
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM screenshots", [], |r| r.get(0))
            .map_err(|e| format!("Count failed: {e}"))?;
        let done: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM screenshots WHERE embedded_at IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .map_err(|e| format!("Count failed: {e}"))?;
        Ok((done, total))
    })
}

/// Forget a screenshot: drops the row, its vector and its FTS entry.
///
/// Returns the path so the caller can delete the file too. The file is *not*
/// removed here -- a database failure must never cost someone a screenshot that
/// is still on disk, so the caller deletes it only after this succeeds.
pub fn forget(path: &str) -> Result<String, String> {
    with_conn(|conn| {
        let id: Option<i64> = conn
            .query_row(
                "SELECT id FROM screenshots WHERE path = ?1",
                params![path],
                |r| r.get(0),
            )
            .ok();
        let Some(id) = id else {
            return Ok(path.to_string());
        };
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Transaction failed: {e}"))?;
        tx.execute("DELETE FROM screenshot_vec WHERE rowid = ?1", params![id])
            .map_err(|e| format!("Vector delete failed: {e}"))?;
        // The FTS triggers fire on the row delete and keep the index in step.
        tx.execute("DELETE FROM screenshots WHERE id = ?1", params![id])
            .map_err(|e| format!("Row delete failed: {e}"))?;
        Ok(path.to_string())
    })
}

/// Turn user input into a literal FTS5 query.
///
/// FTS5 reads `-`, `"`, `*`, `:`, `^` and the bare words OR/NOT/AND/NEAR as
/// query *operators*, so passing raw input through makes `error -debug` or
/// `foo:` a syntax error rather than a search. Splitting on non-alphanumerics
/// discards every operator character, and wrapping each term in double quotes
/// makes it a literal that cannot be reinterpreted.
pub fn fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"", t.to_lowercase()))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

#[tauri::command]
pub async fn screenshot_detail(path: String) -> Result<ScreenshotDetail, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT path, created_at, width, height, source,
                        ocr_text, ocr_boxes, embedded_at, embed_model
                 FROM screenshots WHERE path = ?1",
            )
            .map_err(|e| format!("Prepare failed: {e}"))?;
        stmt.query_row(params![path], |row| {
            let boxes: Option<String> = row.get(6)?;
            let ocr_indexed = boxes.is_some();
            let ocr_word_count = boxes
                .as_deref()
                .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
                .and_then(|v| v.as_array().map(|a| a.len()))
                .unwrap_or(0);
            Ok(ScreenshotDetail {
                path: row.get(0)?,
                created_at: row.get(1)?,
                width: row.get::<_, i64>(2)? as u32,
                height: row.get::<_, i64>(3)? as u32,
                source: row.get(4)?,
                ocr_text: row.get(5)?,
                ocr_word_count,
                ocr_indexed,
                embedded_at: row.get(7)?,
                embed_model: row.get(8)?,
            })
        })
        .map_err(|e| format!("Screenshot not found: {e}"))
    })
}

#[tauri::command]
pub async fn search_text(query: String, limit: Option<u32>) -> Result<Vec<SearchHit>, String> {
    let Some(match_expr) = fts_query(&query) else {
        return Ok(Vec::new());
    };
    let limit = limit.unwrap_or(50).clamp(1, 200) as i64;

    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT s.path, s.created_at, s.width, s.height, s.source,
                        snippet(screenshots_fts, 0, '\u{1f}', '\u{1e}', '…', 24)
                 FROM screenshots_fts
                 JOIN screenshots s ON s.id = screenshots_fts.rowid
                 WHERE screenshots_fts MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )
            .map_err(|e| format!("Prepare failed: {e}"))?;

        let rows = stmt
            .query_map(params![match_expr, limit], |row| {
                Ok(SearchHit::text(
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .map_err(|e| format!("Query failed: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row read failed: {e}"))
    })
}

/// Nearest screenshots to a query vector, by cosine distance.
///
/// `vec0` needs the KNN constraint planned on the table itself: wrapping it in
/// an aliased JOIN hides `k = ?1` from the query planner, which then returns
/// arbitrary rows with distance 0.
#[tauri::command]
pub async fn search_similar(query: String, limit: Option<u32>) -> Result<Vec<SearchHit>, String> {
    let embedder = crate::indexer::embedder().ok_or("embedding server not running")?;
    let vector = embedder.embed_query(&query)?;
    if vector.len() != EMBED_DIM {
        return Err(format!("expected {EMBED_DIM} dims, got {}", vector.len()));
    }
    let limit = limit.unwrap_or(50).clamp(1, 200);
    let bytes = crate::embedder::to_f32_bytes(&vector);

    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT s.path, s.created_at, s.width, s.height, s.source, v.distance
                 FROM screenshot_vec v
                 JOIN screenshots s ON s.id = v.rowid
                 WHERE v.embedding MATCH ?1 AND v.k = ?2
                 ORDER BY v.distance",
            )
            .map_err(|e| format!("Prepare failed: {e}"))?;
        let mut hits = stmt
            .query_map(params![bytes, limit as i64], |row| {
                Ok(SearchHit::visual(
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .map_err(|e| format!("Query failed: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row read failed: {e}"))?;
        let cosines: Vec<f64> = hits
            .iter()
            .map(|h: &SearchHit| h.score.unwrap_or(0.0) as f64)
            .collect();
        for (hit, z) in hits.iter_mut().zip(standardise(&cosines)) {
            hit.score_z = Some(z as f32);
        }
        Ok(hits)
    })
}

/// Convert vec0's L2 distance into a cosine similarity.
///
/// `vec0` defaults to L2 unless the column is declared
/// `distance_metric=cosine`, and this table is declared as plain
/// `vec0(embedding float[768])`. For unit-norm vectors (which ours are; norm
/// measured at 1.0000) the two relate by `L2 = sqrt(2 - 2*cos)`.
///
/// The old display used `1/(1+distance)`, which is not a similarity at all: it
/// squeezed every result into 51-59% regardless of quality. It reported L2
/// 0.70 as "58.8%" when the true cosine was 0.755.
pub fn l2_to_cosine(distance: f64) -> f64 {
    (1.0 - (distance * distance) / 2.0).clamp(-1.0, 1.0)
}

/// Standardise one result set's cosines: `(cos - mean) / std`.
///
/// Fewer than 2 hits or a near-zero spread standardises to all zeros, so the
/// caller falls back to rank order instead of dividing by noise.
fn standardise(cosines: &[f64]) -> Vec<f64> {
    if cosines.len() < 2 {
        return vec![0.0; cosines.len()];
    }
    let mean = cosines.iter().sum::<f64>() / cosines.len() as f64;
    let var = cosines.iter().map(|c| (c - mean) * (c - mean)).sum::<f64>() / cosines.len() as f64;
    let std = var.sqrt();
    if std < 1e-9 {
        return vec![0.0; cosines.len()];
    }
    cosines.iter().map(|c| (c - mean) / std).collect()
}

/// One ranked id list with its raw relevance score. Both arms report
/// "lower is better" (BM25 `rank` is negative; vec0 returns a distance).
fn ranked_scored(
    conn: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<(i64, f64)>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| format!("Prepare failed: {e}"))?;
    let rows = stmt
        .query_map(args, |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?)))
        .map_err(|e| format!("Query failed: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Row read failed: {e}"))
}

/// Rescale one arm so 1.0 is its best hit and 0.0 its worst, with *higher now
/// meaning better*.
///
/// Without this, BM25 (roughly 0..30) and cosine distance (roughly 0..1) cannot
/// be added -- the lexical numbers are simply bigger and would win every query.
/// A single flat score has nothing to rescale.
fn normalise_min_max(scored: &[(i64, f64)]) -> HashMap<i64, f64> {
    if scored.is_empty() {
        return HashMap::new();
    }
    let lo = scored.iter().map(|(_, s)| *s).fold(f64::INFINITY, f64::min);
    let hi = scored.iter().map(|(_, s)| *s).fold(f64::NEG_INFINITY, f64::max);
    let span = hi - lo;
    scored
        .iter()
        .map(|&(id, s)| {
            let norm = if span.abs() < 1e-12 { 1.0 } else { (hi - s) / span };
            (id, norm)
        })
        .collect()
}

/// Convex combination of normalised scores.
///
/// Unlike RRF this keeps *how strongly* each retriever matched, and the weights
/// let one arm outvote the other. Visual is weighted higher because a described
/// scene is a weaker signal than a literal word: the OCR arm returns nothing at
/// all when a word is absent, whereas the visual arm always returns its nearest
/// neighbours, so letting it dominate on ties would bury exact text matches.
///
/// A document missing from one arm takes that arm's floor rather than zero:
/// the 74 text-free screenshots are never in the lexical list at all, and a
/// hard zero would make them unfindable no matter how they look.
fn weighted_fuse(
    lexical: &[(i64, f64)],
    visual: &[(i64, f64)],
    w_lexical: f64,
    w_visual: f64,
) -> Vec<i64> {
    let lex_n = normalise_min_max(lexical);
    let vis_n = normalise_min_max(visual);
    // An arm that returned nothing has no distribution to take a floor from, and
    // min-of-empty is infinity: every document would score inf, tie, and fall
    // through to the id tiebreak, handing back a meaningless order. With no
    // evidence from an arm it should contribute nothing.
    let floor_of = |m: &HashMap<i64, f64>| {
        if m.is_empty() {
            0.0
        } else {
            m.values().cloned().fold(f64::INFINITY, f64::min)
        }
    };
    let lex_floor = floor_of(&lex_n);
    let vis_floor = floor_of(&vis_n);

    let mut ids: Vec<i64> = lex_n.keys().chain(vis_n.keys()).copied().collect();
    ids.sort_unstable();
    ids.dedup();

    let mut fused: Vec<(i64, f64)> = ids
        .into_iter()
        .map(|id| {
            let lex = lex_n.get(&id).copied().unwrap_or(lex_floor);
            let vis = vis_n.get(&id).copied().unwrap_or(vis_floor);
            (id, w_lexical * lex + w_visual * vis)
        })
        .collect();
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.0.cmp(&a.0))
    });
    fused.into_iter().map(|(id, _)| id).collect()
}

/// How much each arm contributes.
///
/// Visual leads, but only slightly. The case for it is that the two arms are
/// not symmetric in coverage: the OCR arm returns *nothing* when a word is
/// absent, while the visual arm always returns its nearest neighbours, so on
/// ties the visual arm would otherwise quietly displace exact text matches.
/// Override with `XREENSHOTY_FUSION_WEIGHTS="0.44,0.56"` (lexical, visual).
fn fusion_weights() -> (f64, f64) {
    match std::env::var("XREENSHOTY_FUSION_WEIGHTS")
        .ok()
        .and_then(|v| {
            let mut it = v.split(',');
            Some((it.next()?.trim().parse().ok()?, it.next()?.trim().parse().ok()?))
        }) {
        Some((a, b)) if a >= 0.0 && b >= 0.0 && a + b > 0.0 => (a, b),
        _ => (0.44, 0.56),
    }
}

/// Hybrid search: BM25 over OCR text and k-NN over image vectors, fused by
/// reciprocal rank.
///
/// The visual arm is optional by design. The model takes a couple of minutes to
/// load and is not running at all before first use, so a cold or failed embedder
/// degrades to plain keyword search instead of failing the whole query.
#[tauri::command]
pub async fn search_hybrid(query: String, limit: Option<u32>) -> Result<Vec<SearchHit>, String> {
    let limit = limit.unwrap_or(50).clamp(1, 200);
    // Over-fetch each arm so a result ranked just outside the final cut-off by
    // one retriever can still be pulled in by agreement with the other.
    let pool = (limit as usize * 4).clamp(50, 400);

    // Visual arm first: embedding is the slow step and decides whether we can
    // run two arms at all.
    let vector = match crate::indexer::embedder() {
        Some(e) => match e.embed_query(&query) {
            Ok(v) if v.len() == EMBED_DIM => Some(crate::embedder::to_f32_bytes(&v)),
            Ok(v) => {
                return Err(format!("expected {EMBED_DIM} dims, got {}", v.len()));
            }
            Err(e) => {
                log::warn!("search: visual arm unavailable, keyword only ({e})");
                None
            }
        },
        None => None,
    };

    let snippet_sql = "snippet(screenshots_fts, 0, '\u{1f}', '\u{1e}', '…', 24)";
    let (order, lexical) = with_conn(|conn| {
        let lexical = match fts_query(&query) {
            Some(m) => ranked_scored(
                conn,
                &format!(
                    "SELECT rowid, rank FROM screenshots_fts
                     WHERE screenshots_fts MATCH ?1 ORDER BY rank LIMIT ?2"
                ),
                &[&m as &dyn rusqlite::ToSql, &(pool as i64) as &dyn rusqlite::ToSql],
            )?,
            None => Vec::new(),
        };

        let visual = match &vector {
            Some(bytes) => ranked_scored(
                conn,
                "SELECT rowid, distance FROM screenshot_vec
                 WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance",
                &[bytes as &dyn rusqlite::ToSql, &(pool as i64) as &dyn rusqlite::ToSql],
            )
            // A missing or empty vector table must not sink the keyword arm.
            .unwrap_or_else(|e| {
                log::warn!("search: vector arm failed, keyword only ({e})");
                Vec::new()
            }),
            None => Vec::new(),
        };

        let order = match std::env::var("XREENSHOTY_FUSION") {
            Ok(v) if v.eq_ignore_ascii_case("rrf") => {
                let ids = |v: &[(i64, f64)]| v.iter().map(|(i, _)| *i).collect::<Vec<_>>();
                rrf_fuse(&ids(&lexical), &ids(&visual), RRF_K)
            }
            _ => {
                let (w_lex, w_vis) = fusion_weights();
                weighted_fuse(&lexical, &visual, w_lex, w_vis)
            }
        };
        Ok((order, lexical))
    })?;

    if order.is_empty() {
        return Ok(Vec::new());
    }

    // Re-read the winning ids for display fields. Snippets only exist on the
    // lexical side, so visual-only hits simply carry none. Ordering is applied
    // in Rust: expressing "ORDER BY fused rank" in SQL would mean a CASE per
    // result, which is harder to read and no faster at 374 rows.
    let placeholders = order.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let lexical_hits: Vec<i64> = lexical.iter().map(|(i, _)| *i).collect();

    with_conn(|conn| {
        let sql = format!(
            "SELECT id, path, created_at, width, height, source,
                    CASE WHEN id IN ({lex_ph}) THEN (SELECT {snippet_sql}
                        FROM screenshots_fts f WHERE f.rowid = id) END
             FROM screenshots WHERE id IN ({placeholders})",
            lex_ph = lexical_hits
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",")
        );

        let mut args: Vec<Box<dyn rusqlite::ToSql>> =
            lexical_hits.iter().map(|id| Box::new(*id) as Box<dyn rusqlite::ToSql>).collect();
        args.extend(order.iter().map(|id| Box::new(*id) as Box<dyn rusqlite::ToSql>));

        let mut stmt = conn.prepare(&sql).map_err(|e| format!("Prepare failed: {e}"))?;
        let params: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(params.as_slice(), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    SearchHit::text(
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ),
                ))
            })
            .map_err(|e| format!("Query failed: {e}"))?;

        let mut by_id = std::collections::HashMap::new();
        for row in rows {
            let (id, hit) = row.map_err(|e| e.to_string())?;
            by_id.insert(id, hit);
        }
        Ok(order
            .iter()
            .filter_map(|id| by_id.remove(id))
            .take(limit as usize)
            .collect())
    })
}

/// Reciprocal Rank Fusion.
///
/// Only *positions* are used, never raw scores. BM25 and cosine live on
/// incompatible scales, so adding them directly just lets whichever has bigger
/// numbers win; `k` flattens the curve so a result that both retrievers like
/// mid-list can beat a lone top hit from one of them.
pub const RRF_K: f64 = 60.0;



/// Fuse ranked id lists (best first) into one ranking.
///
/// Returns ids ordered by fused score, descending.
fn rrf_fuse(lexical: &[i64], visual: &[i64], k: f64) -> Vec<i64> {
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for list in [lexical, visual] {
        for (i, &id) in list.iter().enumerate() {
            *scores.entry(id).or_insert(0.0) += 1.0 / (k + (i + 1) as f64);
        }
    }
    let mut fused: Vec<(i64, f64)> = scores.into_iter().collect();
    // Ties broken by id so paging through results is stable rather than
    // shuffling between identical scores.
    fused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(b.0.cmp(&a.0)));
    fused.into_iter().map(|(id, _)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: (f64, f64) = (0.44, 0.56);

    #[test]
    fn standardise_spreads_a_packed_set() {
        let zs = standardise(&[0.97, 0.95, 0.94, 0.93]);
        assert!(zs[0] > 1.0, "top hit must stand out, got {zs:?}");
        assert!(zs[3] < -1.0, "bottom hit must sink, got {zs:?}");
        let mean: f64 = zs.iter().sum::<f64>() / zs.len() as f64;
        assert!(mean.abs() < 1e-9, "z-scores must centre on zero, got {mean}");
    }

    #[test]
    fn standardise_degrades_gracefully() {
        assert_eq!(standardise(&[]), Vec::<f64>::new());
        assert_eq!(standardise(&[0.95]), vec![0.0]);
        assert_eq!(standardise(&[0.95, 0.95, 0.95]), vec![0.0, 0.0, 0.0]);
    }

    /// The point of the whole change: both arms return the *same order*, but
    /// doc 3 outmatched doc 2 by a wide margin in both. RRF cannot see that and
    /// leaves the order alone; weighted fusion reorders them.
    #[test]
    fn strength_reorders_what_rank_alone_cannot() {
        let lex = vec![(1i64, -20.0), (2, -1.0), (3, -2.0)];
        let vis = vec![(1i64, 0.01), (2, 0.99), (3, 0.50)];

        assert_eq!(rrf_fuse(&[1, 2, 3], &[1, 2, 3], RRF_K), vec![1, 2, 3]);
        assert_eq!(
            weighted_fuse(&lex, &vis, W.0, W.1),
            vec![1, 3, 2],
            "doc 3 matched far more strongly than doc 2 and should overtake it"
        );
    }

    /// When the arms disagree about first place, RRF's mirrored ranks produce an
    /// exact tie and it breaks it arbitrarily. Weighted fusion resolves it.
    #[test]
    fn weighted_fuse_breaks_a_tie_rrf_cannot() {
        // Lexical strongly prefers 1, visual mildly prefers 2.
        let lex = vec![(1i64, -20.0), (2, -2.0), (3, -1.0)];
        let vis = vec![(2i64, 0.05), (1, 0.50), (3, 0.90)];

        // RRF: 1 and 2 tie to the last decimal, so 2 wins on the id tiebreak.
        assert_eq!(rrf_fuse(&[1, 2, 3], &[2, 1, 3], RRF_K), vec![2, 1, 3]);
        // Weighted: doc 1's lexical strength outvotes doc 2's visual strength.
        assert_eq!(weighted_fuse(&lex, &vis, W.0, W.1), vec![1, 2, 3]);
    }

    /// Visual should be able to outvote OCR, but only when it is confident.
    #[test]
    fn visual_is_weighted_slightly_higher() {
        // Doc 1 is the OCR arm's best hit, doc 2 the visual arm's. With visual
        // weighted above OCR, doc 2 takes it.
        let lex = vec![(1i64, -10.0), (2, -9.0)];
        let vis = vec![(1i64, 0.80), (2, 0.10)];
        assert_eq!(weighted_fuse(&lex, &vis, 0.44, 0.56)[0], 2);

        // Flip the weights and the ordering flips back, which is what proves
        // the weight is actually doing the work rather than the ordering.
        assert_eq!(weighted_fuse(&lex, &vis, 0.56, 0.44)[0], 1);
    }

    /// 74 of the screenshots have no OCR text at all, so they are never in the
    /// lexical list. A hard zero would make them unfindable however they look.
    #[test]
    fn a_missing_arm_takes_the_floor_not_zero() {
        let lex = vec![(1i64, -9.0), (2, -8.0)];
        let vis = vec![(2i64, 0.10), (3, 0.12)];
        let fused = weighted_fuse(&lex, &vis, W.0, W.1);
        assert!(
            fused.contains(&3),
            "screenshot 3 has no OCR text and must still be findable"
        );
        // ...and it must not outrank a result both arms like.
        assert_eq!(fused[0], 2, "agreement should still beat a one-sided hit");
    }

    /// With one arm returning nothing, the surviving arm must still decide the
    /// order on its own. min-of-empty is infinity, which previously made every
    /// document tie and fall through to the id tiebreak -- an arbitrary order.
    #[test]
    fn one_empty_arm_still_orders_by_the_other() {
        // vec0 distance: lower is closer, so id 9 at 0.10 is the best match.
        let vis = vec![(9i64, 0.10), (7, 0.50), (8, 0.90)];
        assert_eq!(weighted_fuse(&[], &vis, W.0, W.1), vec![9, 7, 8]);
        let lex = vec![(3i64, -30.0), (1, -5.0), (2, -1.0)];
        assert_eq!(weighted_fuse(&lex, &[], W.0, W.1), vec![3, 1, 2]);
    }

    #[test]
    fn empty_and_degenerate_inputs_do_not_panic() {
        assert!(weighted_fuse(&[], &[], W.0, W.1).is_empty());
        assert_eq!(weighted_fuse(&[(1, 0.0)], &[], W.0, W.1), vec![1]);
        assert_eq!(weighted_fuse(&[], &[(1, 0.0)], W.0, W.1), vec![1]);
        // Every score identical: no span to normalise against.
        let flat = vec![(1i64, -2.0), (2, -2.0), (3, -2.0)];
        assert_eq!(weighted_fuse(&flat, &flat, W.0, W.1).len(), 3);
    }

    /// Both arms report "lower is better" (BM25 `rank` is negative, vec0 returns
    /// a distance), so the *smallest* raw score is the best and must end up
    /// highest after rescaling.
    #[test]
    fn normalise_maps_worst_to_zero_and_best_to_one() {
        let n = normalise_min_max(&[(1, 1.0), (2, 5.0), (3, 3.0)]);
        assert_eq!(n[&1], 1.0, "smallest raw value is the best match");
        assert_eq!(n[&2], 0.0, "largest raw value is the worst match");
        assert!((n[&3] - 0.5).abs() < 1e-9);
    }

    /// Identity check against the documented relation, `L2 = sqrt(2 - 2*cos)`.
    #[test]
    fn l2_converts_back_to_cosine() {
        for cos in [0.0f64, 0.25, 0.5, 0.75, 1.0] {
            let l2 = (2.0 - 2.0 * cos).sqrt();
            assert!((l2_to_cosine(l2) - cos).abs() < 1e-9, "cos={cos}");
        }
        // The two readings the UI used to collapse together are not the same.
        assert!((l2_to_cosine(0.70) - 0.755).abs() < 1e-3);
        assert!((l2_to_cosine(0.95) - 0.549).abs() < 1e-3);
        // Identical vectors sit at cosine 1, not at the old formula's ceiling.
        assert!((l2_to_cosine(0.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn default_weights_favour_visual_slightly() {
        let (lex, vis) = fusion_weights();
        assert!((lex - 0.44).abs() < 1e-9, "got {lex}");
        assert!((vis - 0.56).abs() < 1e-9, "got {vis}");
        assert!(vis > lex, "visual should lead");
    }

    #[test]
    fn fusion_is_symmetric_across_the_two_arms() {
        // 1 sits at lexical #1 / visual #3 and 3 at lexical #3 / visual #1.
        // Their reciprocal sums are algebraically identical, so RRF cannot and
        // does not prefer either -- worth pinning down, because it is exactly
        // the information RRF discards by design.
        let lex = vec![1, 2, 3, 4];
        let vis = vec![3, 2, 1, 4];
        let fused = rrf_fuse(&lex, &vis, RRF_K);

        assert_eq!(fused.len(), 4);
        assert_eq!(fused[3], 4, "worst on both arms must finish last");
        let top = &fused[..2];
        assert!(
            top.contains(&1) && top.contains(&3),
            "the two mirrored positions should tie at the top, got {top:?}"
        );
        assert!(fused.iter().position(|&x| x == 2).unwrap() < 3);
    }

    #[test]
    fn a_single_arm_still_ranks_sensibly() {
        // With one arm empty this degrades to that arm's own order.
        let lex = vec![7, 8, 9];
        assert_eq!(rrf_fuse(&lex, &[], RRF_K), lex);
        assert_eq!(rrf_fuse(&[], &lex, RRF_K), lex);
        assert!(rrf_fuse(&[], &[], RRF_K).is_empty());
    }

    #[test]
    fn both_arms_empty_is_not_an_error() {
        assert_eq!(rrf_fuse(&[], &[], RRF_K), Vec::<i64>::new());
    }

    /// The defining property: a result both retrievers like outranks one that
    /// only a single retriever liked.
    #[test]
    fn consensus_is_what_fusion_rewards() {
        let lex = vec![10, 99, 98];
        let vis = vec![98, 99, 10];
        let fused = rrf_fuse(&lex, &vis, RRF_K);
        // 99 is mid on both lists, 98 is #2 and #1, 10 is #1 and #3.
        assert_eq!(fused.first(), Some(&98));
        assert!(fused.iter().position(|&x| x == 10).unwrap() > 0);
    }

    /// Every one of these would be a syntax error, not a search, if handed to
    /// FTS5 unescaped.
    #[test]
    fn query_is_sanitised_into_literal_terms() {
        assert_eq!(fts_query("firefox").as_deref(), Some("\"firefox\""));
        assert_eq!(fts_query("Error -debug").as_deref(), Some("\"error\" OR \"debug\""));
        assert_eq!(fts_query("a:b*c").as_deref(), Some("\"a\" OR \"b\" OR \"c\""));
        assert_eq!(fts_query("OR NOT AND").as_deref(), Some("\"or\" OR \"not\" OR \"and\""));
        assert_eq!(fts_query("MiXeD Case").as_deref(), Some("\"mixed\" OR \"case\""));
        assert_eq!(fts_query("   ").as_deref(), None);
        assert_eq!(fts_query("!!!").as_deref(), None);
    }

    #[test]
    fn empty_query_yields_no_results() {
        assert_eq!(fts_query(""), None);
    }

    /// Exercises the import path against a temp dir of real PNGs: the dedupe rule,
    /// the metadata columns, and the IHDR header read (which replaced decoding
    /// the whole image and is easy to get byte-offset wrong).
    #[tokio::test]
    async fn import_is_idempotent_and_reads_dimensions() {
        let _guard = test_lock();
        let dir = std::env::temp_dir().join("xreenshoty-db-test");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        screenshots::image::RgbaImage::new(7, 5).save(&png).unwrap();

        let db = db_path().unwrap().with_extension("test.db");
        // Must clear the WAL sidecars too: deleting only the main file leaves
        // `-wal`/`-shm` behind, and SQLite replays them on reopen, so the rows
        // from the previous run come back and the import looks like a no-op.
        for suffix in ["", "-wal", "-shm"] {
            std::fs::remove_file(format!("{}{suffix}", db.display())).ok();
        }
        std::env::set_var("XREENSHOTY_TEST_DB", &db);

        let first = import_existing(Some(dir.to_string_lossy().into_owned()))
            .await
            .unwrap();
        let second = import_existing(Some(dir.to_string_lossy().into_owned()))
            .await
            .unwrap();
        assert_eq!(first, 1, "first import should add the one png");
        assert_eq!(second, 0, "second import must not duplicate");

        let row: (u32, u32) = with_conn(|conn| {
            conn.query_row("SELECT width, height FROM screenshots", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map_err(|e| format!("Query failed: {e}"))
        })
        .unwrap();
        assert_eq!(row, (7, 5), "IHDR header read must match the real size");

        for suffix in ["", "-wal", "-shm"] {
            std::fs::remove_file(format!("{}{suffix}", db.display())).ok();
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// BM6: deleting a PNG outside the app must remove the row *and* both search
/// indexes. A surviving `screenshots_fts` entry is invisible until it outranks
/// a real hit; a surviving `vec0` row is worse, it skews every KNN query.
#[test]
fn prune_removes_gone_rows_and_their_search_entries() {
    let _guard = test_lock();
    let db = std::env::temp_dir().join("xreenshoty-prune.db");
    for suffix in ["", "-wal", "-shm"] {
        std::fs::remove_file(format!("{}{suffix}", db.display())).ok();
    }
    std::env::set_var("XREENSHOTY_TEST_DB", &db);
    with_conn(|_| Ok(())).unwrap();

    let dir = std::env::temp_dir().join("xreenshoty-prune-dir");
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    let kept = dir.join("kept.png");
    let gone = dir.join("gone.png");
    screenshots::image::RgbaImage::new(4, 4).save(&kept).unwrap();
    screenshots::image::RgbaImage::new(4, 4).save(&gone).unwrap();

    let kept_id = with_conn(|conn| {
        conn.execute(
            "INSERT INTO screenshots (path, created_at, width, height, source, ocr_text, ocr_boxes)
             VALUES (?1, 1, 4, 4, 'capture', 'segmentation fault in kernel', '[]')",
            params![kept.to_string_lossy()],
        ).map_err(|e| format!("insert kept: {e}"))?;
        conn.execute(
            "INSERT INTO screenshots (path, created_at, width, height, source, ocr_text, ocr_boxes)
             VALUES (?1, 2, 4, 4, 'capture', 'permission denied by sudo', '[]')",
            params![gone.to_string_lossy()],
        ).map_err(|e| format!("insert gone: {e}"))?;
        let id: i64 = conn
            .query_row("SELECT id FROM screenshots WHERE path = ?1", params![gone.to_string_lossy()], |r| r.get(0))
            .map_err(|e| format!("select: {e}"))?;
        conn.execute(
            "INSERT INTO screenshot_vec(rowid, embedding) VALUES (?1, ?2)",
            params![id, crate::embedder::to_f32_bytes(&vec![0.25f32; EMBED_DIM])],
        ).map_err(|e| format!("insert vec: {e}"))?;
        Ok(id)
    }).unwrap();

    // Nothing has vanished yet.
    assert_eq!(prune_missing().unwrap(), 0);

    // Driven through `import_existing`, the path that actually runs on launch:
    // a unit test of `prune_missing` alone would pass even if nobody called it.
    // `block_on` rather than `#[tokio::test]`: the `test_lock` guard must not be
    // held across an await.
    std::fs::remove_file(&gone).unwrap();
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(import_existing(Some(dir.to_string_lossy().into_owned())))
        .unwrap();

    let (rows, fts, vec): (i64, i64, i64) = with_conn(|conn| {
        let rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM screenshots WHERE id = ?1",
                params![kept_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("count row: {e}"))?;
        let fts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM screenshots_fts WHERE screenshots_fts MATCH 'sudo' AND rowid = ?1",
                params![kept_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("count fts: {e}"))?;
        let vec: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM screenshot_vec WHERE rowid = ?1",
                params![kept_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("count vec: {e}"))?;
        Ok((rows, fts, vec))
    }).unwrap();

    assert_eq!((rows, fts, vec), (0, 0, 0), "row, FTS entry and vector must all be gone");

    // The surviving screenshot is untouched.
    let left: i64 = with_conn(|conn| {
        conn.query_row("SELECT COUNT(*) FROM screenshots WHERE path = ?1", params![kept.to_string_lossy()], |r| r.get(0))
            .map_err(|e| format!("count: {e}"))
    }).unwrap();
    assert_eq!(left, 1, "a file that still exists must not be pruned");

    for suffix in ["", "-wal", "-shm"] {
        std::fs::remove_file(format!("{}{suffix}", db.display())).ok();
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// BH1: rows owing OCR must come first, or a library with no embedding
    /// server keeps re-returning the same oldest rows and never reaches the
    /// later ones. `ocr_boxes` is what marks OCR "attempted" -- `ocr_text` is
    /// legitimately empty for text-free screenshots.
    #[test]
    fn pending_puts_rows_that_need_ocr_first() {
        let _guard = test_lock();
        let db = std::env::temp_dir().join("xreenshoty-pending-order.db");
        for suffix in ["", "-wal", "-shm"] {
            std::fs::remove_file(format!("{}{suffix}", db.display())).ok();
        }
        std::env::set_var("XREENSHOTY_TEST_DB", &db);
        with_conn(|_| Ok(())).unwrap();

        with_conn(|conn| {
            // oldest row: OCR'd, never embedded -> embed-only
            conn.execute(
                "INSERT INTO screenshots (path, created_at, width, height, source, ocr_text, ocr_boxes, embedded_at)
                 VALUES ('/tmp/old.png', 1, 1, 1, 'capture', 'hello', '[]', NULL)",
                [],
            ).map_err(|e| format!("insert: {e}"))?;
            // newer rows: no OCR yet, so they must outrank the embed-only row
            for (created_at, name) in [(2i64, "new-a"), (3, "new-b")] {
                conn.execute(
                    "INSERT INTO screenshots (path, created_at, width, height, source)
                     VALUES (?1, ?2, 1, 1, 'capture')",
                    params![format!("/tmp/{name}.png"), created_at],
                ).map_err(|e| format!("insert: {e}"))?;
            }
            Ok(())
        })
        .unwrap();

        let rows = pending(25).unwrap();
        let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["/tmp/new-a.png", "/tmp/new-b.png", "/tmp/old.png"],
            "OCR-needing rows must be returned before the stuck embed-only row"
        );
        assert!(rows[0].needs_ocr && !rows[2].needs_ocr);

        // A skipped row stays pending on purpose, so it is retried once a model
        // server is actually available.
        mark_embedding_skipped(rows[2].id).unwrap();
        assert!(
            pending(25).unwrap().iter().any(|r| r.path == "/tmp/old.png"),
            "a skipped row must stay pending or it never gets embedded"
        );

        for suffix in ["", "-wal", "-shm"] {
            std::fs::remove_file(format!("{}{suffix}", db.display())).ok();
        }
    }
}
