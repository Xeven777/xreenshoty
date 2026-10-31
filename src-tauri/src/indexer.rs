/// Whether to immediately start the next batch.
///
/// Must key off work *done*, never the row count. A full batch of rows that
/// could not be processed (model still loading) comes straight back on the next
/// query, so chaining on row count spins the worker against the DB.
fn should_chain(processed: usize, rows_were_full: bool, batch: usize) -> bool {
    processed >= batch && rows_were_full
}

/// Whether the face geometry changed enough to be worth regrouping.
///
/// A recluster rewrites `cluster_id` for every face and reconciles the name
/// rows, so running it after a pass that stored nothing -- every screenshot was
/// face-free, or every store failed -- is pure churn. It also logged
/// "reclustered N faces" after a batch that wrote nothing, which reads as
/// progress and hides the failure directly above it.
fn should_recluster(faces_stored: usize) -> bool {
    faces_stored > 0
}

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::db;
use crate::embedder::{Config, Embedder};
use crate::face;
use crate::ocr;

/// How often the worker wakes up on its own if nothing pokes it.
/// Embeddings slower than this are logged individually; the batch average
/// covers the rest. `XREENSHOTY_SLOW_IMAGE_SECS=0` logs every single image.
fn slow_image_threshold() -> f32 {
    std::env::var("XREENSHOTY_SLOW_IMAGE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2.5)
}

const IDLE_POLL: Duration = Duration::from_secs(20);
/// While paused, wake this often so a resume is felt immediately.
const PAUSE_POLL: Duration = Duration::from_millis(400);
/// Rows handled per pass.
const BATCH: u32 = 25;

/// How long OCR stays out of the way after a batch failed for environmental
/// reasons.
///
/// The failed rows are left unstamped, so a fixed environment picks them up
/// later -- but they sort first, and re-offering them every poll would pin
/// them at the head of the queue and starve the embedding pass for good. A
/// short backoff lets the rest of the library catch up first.
static OCR_BACKOFF_UNTIL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

const OCR_BACKOFF: Duration = Duration::from_secs(300);

fn millis_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Whether the OCR stage can run this pass.
///
/// False when the environment is not installed, and false for a while after a
/// batch failed. The rows that owe text are then not offered at all, so they
/// cannot sit at the head of the queue and starve the embedding pass.
fn ocr_available() -> bool {
    if !ocr::env_ready() {
        return false;
    }
    millis_now() >= OCR_BACKOFF_UNTIL.load(Ordering::Relaxed)
}

/// Set once at startup so `save_png` can nudge the worker without every
/// capture command having to take an `AppHandle` parameter it would only
/// forward.
static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();
static EMBEDDER: std::sync::OnceLock<Arc<Embedder>> = std::sync::OnceLock::new();
static INDEXER: std::sync::OnceLock<Arc<Indexer>> = std::sync::OnceLock::new();

fn indexer() -> Option<Arc<Indexer>> {
    INDEXER.get().cloned()
}

/// The embedder owned by the running indexer, so search commands can reach it
/// without every one of them having to thread an `AppHandle` through.
pub fn embedder() -> Option<Arc<Embedder>> {
    EMBEDDER.get().cloned()
}

/// Kill the embedding server this process spawned, at app exit.
///
/// The `Embedder` is held by a process-lifetime `OnceLock`, so the last `Arc`
/// never drops and `Drop` would never fire. Called after
/// `tauri::Builder::run` returns, the one point guaranteed to happen exactly
/// once at shutdown.
pub fn shutdown() {
    if let Some(embedder) = embedder() {
        embedder.shutdown();
    }
}

/// Face indexing opt-in. Rust holds no settings state (AGENTS.md): the
/// frontend owns the toggle in `localStorage` and pushes it here on load and
/// on every change, exactly like `set_scan_folders`. Off by default.
static FACE_INDEXING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn face_indexing_enabled() -> bool {
    FACE_INDEXING.load(Ordering::Relaxed)
}

#[tauri::command]
pub async fn set_face_indexing(enabled: bool) -> Result<(), String> {
    FACE_INDEXING.store(enabled, Ordering::Relaxed);
    if enabled {
        poke();
    }
    Ok(())
}

/// Snapshot for the UI. `paused` is read here rather than tracked separately so
/// the frontend and the worker cannot disagree about the current state.
#[derive(serde::Serialize, Clone)]
pub struct IndexStatus {
    pub embedded: i64,
    pub ocred: i64,
    pub total: i64,
    pub paused: bool,
    pub server_ready: bool,
}

#[tauri::command]
pub async fn index_status() -> Result<IndexStatus, String> {
    // `is_ready` is a synchronous HTTP call (800 ms timeout) and the DB read is
    // blocking; neither belongs on an async worker (BM2).
    crate::screenshot::offload(index_status_blocking).await
}

fn index_status_blocking() -> Result<IndexStatus, String> {
    let p = db::index_progress()?;
    let idx = indexer();
    Ok(IndexStatus {
        embedded: p.embedded,
        ocred: p.ocred,
        total: p.total,
        paused: idx
            .as_ref()
            .is_some_and(|i| i.paused.load(Ordering::Relaxed)),
        server_ready: idx.as_ref().is_some_and(|i| i.embedder.is_ready()),
    })
}

/// Pause or resume. Takes effect after the batch in flight finishes, so a
/// half-written vector is never left behind.
#[tauri::command]
pub async fn index_pause(paused: bool) -> Result<(), String> {
    let idx = indexer().ok_or("indexer not running")?;
    idx.paused.store(paused, Ordering::Relaxed);
    // Without this the worker stays parked in `select!` until IDLE_POLL expires,
    // so a resume took up to 20s to be noticed.
    idx.wake.notify_one();
    log::info!("indexer: {}", if paused { "paused" } else { "resumed" });
    Ok(())
}

pub fn poke() {
    let Some(app) = APP.get() else { return };
    if let Some(indexer) = app.try_state::<Arc<Indexer>>() {
        indexer.wake.notify_one();
    }
}

pub struct Indexer {
    embedder: Arc<Embedder>,
    wake: tokio::sync::Notify,
    /// Set by the UI. Checked at the top of the loop so a pause never abandons
    /// a batch half-written.
    paused: std::sync::atomic::AtomicBool,
}

impl Indexer {
    pub fn start(app: AppHandle) {
        let _ = APP.set(app.clone());
        let config = Config::discover();
        let embedder = Arc::new(Embedder::new(config));
        let _ = EMBEDDER.set(embedder.clone());

        // Warm the model while the user is still taking screenshots. Loading
        // takes ~2 minutes, so this must not block app startup.
        if let Err(e) = embedder.start() {
            log::warn!("embedding server not started ({e}); semantic search will retry");
        } else {
            let warm = embedder.clone();
            // spawn_blocking, not spawn: `wait_ready` is a synchronous poll with
            // `thread::sleep` in it, so on the async pool it would hold a runtime
            // worker for up to five minutes.
            tauri::async_runtime::spawn_blocking(move || {
                if warm.wait_ready(Duration::from_secs(300)) {
                    log::info!("embedding server ready");
                } else {
                    log::warn!("embedding server did not become ready in 5 minutes");
                }
            });
        }

        let indexer = Arc::new(Self {
            embedder,
            wake: tokio::sync::Notify::new(),
            paused: std::sync::atomic::AtomicBool::new(false),
        });
        app.manage(indexer.clone());
        let _ = INDEXER.set(indexer.clone());

        // Existing rows (a first run, or a previous session) should start
        // indexing immediately rather than after one idle interval.
        indexer.wake.notify_one();
        tauri::async_runtime::spawn(indexer.run(app));
    }

    /// Recluster after a face batch. Sync and fast (millisecond-scale), so it
    /// runs inline — no spawn_blocking needed. Logs the outcome; errors
    /// surface to the caller for the batch log line.
    fn recluster_quietly(&self) -> Result<usize, String> {
        let n = db::recluster_faces()?;
        if n > 0 {
            log::info!("indexer: reclustered {n} faces");
        }
        Ok(n)
    }

    async fn run(self: Arc<Self>, app: AppHandle) {
        loop {
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(IDLE_POLL) => {}
            }

            // Yield first: a burst of captures pokes us repeatedly, and this
            // keeps that from starving the rest of the runtime.
            tokio::task::yield_now().await;

            // Checked at the top so pausing still lets the current batch
            // finish; nothing half-written is abandoned.
            if self.paused.load(Ordering::Relaxed) {
                tokio::time::sleep(PAUSE_POLL).await;
                continue;
            }

            let label = self.embedder.model_label();
            // Opt-in, off by default: the frontend pushes the Settings toggle
            // through `set_face_indexing` (same shape as `set_scan_folders`).
            // `None` means `pending()` offers no face work at all.
            let face_label = face_indexing_enabled().then_some(face::FACE_MODEL);
            // OCR work is only offered when it can actually run: a row that
            // owes text sorts first, so offering rows the environment cannot
            // handle would starve the embedding pass for good.
            let ocr_ready = ocr_available();
            let rows = match db::pending(BATCH, &label, ocr_ready, face_label) {
                Ok(rows) => rows,
                Err(e) => {
                    log::warn!("indexer: could not read pending work: {e}");
                    continue;
                }
            };
            let rows_were_full = rows.len() >= BATCH as usize;
            if rows.is_empty() {
                continue;
            }

            let ready = self.embedder.is_ready();
            if !ready {
                log::info!(
                    "indexer: embedding server not ready, OCR-only this pass ({} rows)",
                    rows.len()
                );
            }

            let batch_started = std::time::Instant::now();
            // Count screenshots, not stages: one image can be OCR'd *and*
            // embedded in the same pass, and reporting both as "2" read as if
            // two images had been handled.
            let mut touched: std::collections::HashSet<i64> = std::collections::HashSet::new();
            let mut faces_stored: usize = 0;
            let _ = &app;

            // --- OCR: one subprocess for the whole batch, Indra OCR. ---
            // The engine loads its models once and reads the batch itself, so
            // a pass costs one interpreter start, not one per screenshot.
            let ocr_rows: Vec<_> = rows.iter().filter(|r| r.needs_ocr).collect();
            if !ocr_rows.is_empty() {
                let paths: Vec<String> = ocr_rows.iter().map(|r| r.path.clone()).collect();
                match ocr::run_batch(&paths) {
                    Ok(results) => {
                        for (row, outcome) in ocr_rows.iter().zip(results) {
                            match outcome {
                                Ok(result) => {
                                    let words = serde_json::to_string(&result.words)
                                        .unwrap_or_else(|_| "[]".into());
                                    match db::set_ocr(row.id, &result.text, &words) {
                                        // Any successful `set_ocr` is progress,
                                        // including the empty-text case:
                                        // `ocr_boxes` is written either way, so
                                        // the row will not be offered again.
                                        // Counting only non-empty text meant a
                                        // batch of text-free screenshots
                                        // reported no progress and stalled the
                                        // chain.
                                        Ok(()) => {
                                            touched.insert(row.id);
                                        }
                                        Err(e) => log::warn!(
                                            "indexer: storing OCR for {} failed: {e}",
                                            row.path
                                        ),
                                    }
                                }
                                Err(e) => {
                                    log::warn!("indexer: OCR failed for {}: {e}", row.path);
                                    // Mark attempted even on failure. `pending()`
                                    // keys off `ocr_boxes`, so leaving it NULL
                                    // would re-OCR a hopeless image forever --
                                    // paying the full timeout again on every
                                    // idle poll, which is the stall this is
                                    // guarding against.
                                    let _ = db::set_ocr(row.id, "", "[]");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // A batch-level failure is environmental, not per-row:
                        // log once, leave the rows unstamped so a fixed
                        // environment picks them up, and back off so they do
                        // not pin the head of the queue.
                        log::warn!("indexer: OCR batch failed: {e}");
                        OCR_BACKOFF_UNTIL.store(
                            millis_now() + OCR_BACKOFF.as_millis() as u64,
                            Ordering::Relaxed,
                        );
                    }
                }
            }

            // --- Embeddings: one request for the whole batch. ---
            let to_embed: Vec<_> = rows.iter().filter(|r| r.needs_embedding).collect();
            if !to_embed.is_empty() && !ready {
                for row in &to_embed {
                    let _ = db::mark_embedding_skipped(row.id);
                }
            } else {
                // One image per request on purpose: batching 8 into one call
                // measured 0.96x -- no better than sequential, because the GPU
                // is already saturated and a batch just delays the response
                // until all of them are done.
                let threshold = slow_image_threshold();
                for row in &to_embed {
                    let started = std::time::Instant::now();
                    match self.embed_one(row, &label) {
                        Ok(()) => {
                            touched.insert(row.id);
                        }
                        Err(e) => {
                            log::warn!("indexer: embedding {} failed: {e}", row.path);
                            // Permanent failures must not be re-queued at full
                            // speed: back off so one broken image cannot starve
                            // the rest of the library (BM4).
                            match db::record_embed_failure(row.id) {
                                Ok(n) => log::warn!(
                                    "indexer: backing off {} after {n} attempt(s)",
                                    row.path.rsplit('/').next().unwrap_or(&row.path)
                                ),
                                Err(e) => {
                                    log::warn!("indexer: recording failure failed: {e}")
                                }
                            }
                            continue;
                        }
                    }
                    let took = started.elapsed().as_secs_f32();
                    // Per-image timings only for outliers: a full backfill is
                    // ~370 lines and says nothing. The slow ones are the point.
                    if took > threshold {
                        log::warn!(
                            "indexer: slow embed {took:.2}s for {}",
                            row.path.rsplit('/').next().unwrap_or(&row.path)
                        );
                    }
                }
            }

            // --- Faces: one subprocess for the whole batch, after embeddings. ---
            // Ready means venv + script + models; without it the rows stay
            // untouched so they are re-offered when the environment is fixed.
            let to_face: Vec<_> = rows.iter().filter(|r| r.needs_faces).collect();
            if !to_face.is_empty() && face_label.is_some() {
                if face::venv_ready() && face::models_present() {
                    let paths: Vec<String> =
                        to_face.iter().map(|r| r.path.clone()).collect();
                    // A pass over a large library takes a while; say so, so the
                    // log distinguishes "running" from "wedged".
                    log::info!("indexer: face pass over {} screenshot(s)", paths.len());
                    match face::extract(&paths) {                        Ok(all) => {
                            for (row, hits) in to_face.iter().zip(all.iter()) {
                                match db::set_faces(row.id, hits, face::FACE_MODEL) {
                                    Ok(()) => {
                                        touched.insert(row.id);
                                        faces_stored += 1;
                                        if !hits.is_empty() {
                                            log::info!(
                                                "indexer: {} face(s) in {}",
                                                hits.len(),
                                                row.path.rsplit('/').next().unwrap_or(&row.path)
                                            );
                                        }
                                    }
                                    Err(e) => log::warn!(
                                        "indexer: face store failed for {}: {e}",
                                        row.path
                                    ),
                                }
                            }
                        }
                        Err(e) => {
                            // Batch-level failure is environmental, not per-row:
                            // log once and leave the rows unstamped so a fixed
                            // env picks them up on a later pass.
                            log::warn!("indexer: face batch failed: {e}");
                        }
                    }
                    // New faces change the geometry: recluster once per batch
                    // that actually stored one, not per row. Cheap and
                    // idempotent, but pointless after a pass that wrote nothing.
                    if should_recluster(faces_stored) {
                        if let Err(e) = self.recluster_quietly() {
                            log::warn!("indexer: recluster failed: {e}");
                        }
                    }
                } else {
                    // Warn, not debug: the log target filters at Info, so a
                    // missing venv or ONNX model disabled the entire face
                    // feature with nothing at all in the log — and the People
                    // view just showed a progress bar that never moved.
                    let missing = [
                        (!face::venv_ready()).then_some("face-env venv"),
                        (!face::models_present()).then_some("ONNX models"),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" and ");
                    log::warn!(
                        "indexer: face env incomplete ({missing} missing), deferring face pass for {} screenshot(s)",
                        to_face.len()
                    );
                }
            }

            if let Ok(p) = db::index_progress() {
                let _ = app.emit(
                    "index-progress",
                    IndexStatus {
                        embedded: p.embedded,
                        ocred: p.ocred,
                        total: p.total,
                        paused: self.paused.load(Ordering::Relaxed),
                        server_ready: ready,
                    },
                );
            }

            let (done, total) = db::indexed_count().unwrap_or((0, 0));
            log::info!(
                "indexer: {} screenshot(s) in {:.1}s, {done}/{total} embedded",
                touched.len(),
                batch_started.elapsed().as_secs_f32()
            );

            // Chain to the next batch only when a full batch actually got
            // processed. If nothing moved (e.g. the model is still loading) the
            // same rows come straight back, and re-notifying here spins the
            // worker in a tight loop against the DB.
            if should_chain(touched.len(), rows_were_full, BATCH as usize) {
                self.wake.notify_one();
            }
            let _ = &app;
        }
    }

    /// Embed one screenshot and store the vector.
    fn embed_one(&self, row: &db::Pending, label: &str) -> Result<(), String> {
        let png = std::fs::read(&row.path).map_err(|e| format!("read failed: {e}"))?;
        let vector = self.embedder.embed_image(&png)?;
        if vector.len() != db::EMBED_DIM {
            return Err(format!(
                "expected {} dims, got {}",
                db::EMBED_DIM,
                vector.len()
            ));
        }
        db::set_embedding(row.id, &crate::embedder::to_f32_bytes(&vector), label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_batch_that_did_no_work_must_not_chain() {
        // The busy-loop: 25 rows, none processable, chained on row count.
        assert!(!should_chain(0, true, 25));
        assert!(!should_chain(3, true, 25));
        // Genuinely drained a full batch -> keep going immediately.
        assert!(should_chain(25, true, 25));
        assert!(should_chain(26, true, 25));
        // Partial batch -> wait for the poll, nothing is known to be waiting.
        assert!(!should_chain(30, false, 25));
    }

    #[test]
    fn a_face_batch_that_stored_nothing_must_not_recluster() {
        // The failing-store loop: rows offered, nothing written, yet every pass
        // rewrote all clusters and logged "reclustered 288 faces".
        assert!(!should_recluster(0));
        assert!(should_recluster(1));
        assert!(should_recluster(25));
    }

    #[test]
    fn slow_threshold_defaults_and_overrides() {
        unsafe {
            std::env::remove_var("XREENSHOTY_SLOW_IMAGE_SECS");
        }
        assert_eq!(slow_image_threshold(), 2.5);
        unsafe {
            std::env::set_var("XREENSHOTY_SLOW_IMAGE_SECS", "0");
        }
        assert_eq!(slow_image_threshold(), 0.0);
        unsafe {
            std::env::remove_var("XREENSHOTY_SLOW_IMAGE_SECS");
        }
    }

    #[test]
    fn batch_and_poll_are_sane() {
        assert!(BATCH > 0 && BATCH <= 200);
        assert!(IDLE_POLL >= Duration::from_secs(1));
    }
}
