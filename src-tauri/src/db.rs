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
        "CREATE VIRTUAL TABLE IF NOT EXISTS screenshot_vec USING vec0(embedding float[{EMBED_DIM}]);"
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
#[tauri::command]
pub async fn import_existing(dir: Option<String>) -> Result<usize, String> {
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

/// Rows that are not fully indexed yet, oldest first so the original backfill
/// fills in a predictable order. `limit` keeps each pass bounded.
pub fn pending(limit: u32) -> Result<Vec<Pending>, String> {
    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, path,
                        (ocr_boxes IS NULL) AS needs_ocr,
                        (embedded_at IS NULL) AS needs_embedding
                 FROM screenshots
                 WHERE needs_ocr OR needs_embedding
                 ORDER BY created_at ASC
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

/// Marked done even when embedding was skipped, so OCR-only rows stop being
/// retried forever while the model server is unavailable.
pub fn mark_embedding_skipped(id: i64) -> Result<(), String> {
    with_conn(|conn| {
        conn.execute(
            "UPDATE screenshots SET embed_model = 'skipped' WHERE id = ?1 AND embedded_at IS NULL",
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
pub async fn search_text(query: String, limit: Option<u32>) -> Result<Vec<Screenshot>, String> {
    let Some(match_expr) = fts_query(&query) else {
        return Ok(Vec::new());
    };
    let limit = limit.unwrap_or(50).clamp(1, 200) as i64;

    with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT s.path, s.created_at, s.width, s.height, s.source
                 FROM screenshots_fts
                 JOIN screenshots s ON s.id = screenshots_fts.rowid
                 WHERE screenshots_fts MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )
            .map_err(|e| format!("Prepare failed: {e}"))?;

        let rows = stmt
            .query_map(params![match_expr, limit], |row| {
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

/// Nearest screenshots to a query vector, by cosine distance.
///
/// `vec0` needs the KNN constraint planned on the table itself: wrapping it in
/// an aliased JOIN hides `k = ?1` from the query planner, which then returns
/// arbitrary rows with distance 0.
#[tauri::command]
pub async fn search_similar(query: String, limit: Option<u32>) -> Result<Vec<Screenshot>, String> {
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
        let rows = stmt
            .query_map(params![bytes, limit as i64], |row| {
                Ok((
                    Screenshot {
                        path: row.get(0)?,
                        created_at: row.get(1)?,
                        width: row.get::<_, i64>(2)? as u32,
                        height: row.get::<_, i64>(3)? as u32,
                        source: row.get(4)?,
                    },
                    row.get::<_, f64>(5)?,
                ))
            })
            .map_err(|e| format!("Query failed: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Row read failed: {e}"))
            .map(|v| v.into_iter().map(|(s, _)| s).collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
