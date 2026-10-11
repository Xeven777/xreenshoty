use std::path::PathBuf;

/// Whether to immediately start the next batch.
///
/// Must key off work *done*, never the row count. A full batch of rows that
/// could not be processed (model still loading) comes straight back on the next
/// query, so chaining on row count spins the worker against the DB.
fn should_chain(processed: usize, rows_were_full: bool, batch: usize) -> bool {
    processed >= batch && rows_were_full
}

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::db;
use crate::embedder::{Config, Embedder};
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
/// Concurrent tesseract subprocesses. Each is CPU-bound and we have 12 cores,
/// but leaving most of them free keeps the desktop responsive.
const OCR_PARALLEL: usize = 4;

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
    let p = db::index_progress()?;
    let idx = indexer();
    Ok(IndexStatus {
        embedded: p.embedded,
        ocred: p.ocred,
        total: p.total,
        paused: idx.as_ref().is_some_and(|i| i.paused.load(Ordering::Relaxed)),
        server_ready: idx.as_ref().is_some_and(|i| i.embedder.is_ready()),
    })
}

/// Pause or resume. Takes effect after the batch in flight finishes, so a
/// half-written vector is never left behind.
#[tauri::command]
pub async fn index_pause(paused: bool) -> Result<(), String> {
    let idx = indexer().ok_or("indexer not running")?;
    idx.paused.store(paused, Ordering::Relaxed);
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
            tauri::async_runtime::spawn(async move {
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

            let rows = match db::pending(BATCH) {
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
            let _ = &app;

            // --- OCR: tesseract is a subprocess, so these run concurrently. ---
            let ocr_rows: Vec<_> = rows.iter().filter(|r| r.needs_ocr).collect();
            for chunk in ocr_rows.chunks(OCR_PARALLEL) {
                let results: Vec<Result<ocr::OcrResult, String>> = std::thread::scope(|scope| {
                    let handles: Vec<_> = chunk
                        .iter()
                        .map(|row| {
                            let path = PathBuf::from(&row.path);
                            scope.spawn(move || ocr::run(&path))
                        })
                        .collect();
                    handles
                        .into_iter()
                        .map(|h| h.join().unwrap_or_else(|_| Err("OCR thread panicked".into())))
                        .collect()
                });

                for (row, outcome) in chunk.iter().zip(results) {
                    match outcome {
                        Ok(result) => {
                            let words =
                                serde_json::to_string(&result.words).unwrap_or_else(|_| "[]".into());
                            match db::set_ocr(row.id, &result.text, &words) {
                                Ok(()) if !result.text.is_empty() => {
                                    touched.insert(row.id);
                                }
                                Ok(()) => {}
                                Err(e) => {
                                    log::warn!("indexer: storing OCR for {} failed: {e}", row.path)
                                }
                            }
                        }
                        Err(e) => log::warn!("indexer: OCR failed for {}: {e}", row.path),
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
                let label = Config::model_label();
                let threshold = slow_image_threshold();
                for row in &to_embed {
                    let started = std::time::Instant::now();
                    match self.embed_one(row, label) {
                        Ok(()) => {
                            touched.insert(row.id);
                        }
                        Err(e) => {
                            log::warn!("indexer: embedding {} failed: {e}", row.path);
                            let _ = db::mark_embedding_skipped(row.id);
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
        db::set_embedding(
            row.id,
            &crate::embedder::to_f32_bytes(&vector),
            label,
        )
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

    use super::*;

    #[test]
    fn batch_and_poll_are_sane() {
        assert!(BATCH > 0 && BATCH <= 200);
        assert!(IDLE_POLL >= Duration::from_secs(1));
    }
}