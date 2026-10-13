//! Watch the save folder so files dropped in from outside the app are indexed.
//!
//! Pasting an image, dragging one in from a browser, or `cp`-ing a batch over
//! all bypass the capture path entirely, so nothing called `save_png` and
//! nothing poked the indexer. Before this, such a file stayed invisible until
//! the next launch (or a save-folder change, the only other callers of
//! `import_existing`), and a format `import_existing` skipped stayed invisible
//! forever.
//!
//! The watcher deliberately does not parse or index anything itself: it notices
//! that *something* changed, then runs the same `import_existing` +
//! `indexer::poke()` pair the launch path uses. One reconciliation path means
//! the file-watching route cannot drift from the launch route.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

/// How long to keep collecting events before acting on them.
///
/// Editors and clipboard tools write a file in several steps (create, then
/// write, then rename), and saving a batch of images fires one event per file.
/// Acting on each event would run a full directory scan per file, so events are
/// coalesced: every new event restarts the window, and one reconcile runs once
/// the folder has been quiet for this long.
const SETTLE: Duration = Duration::from_millis(400);

/// Start watching `dir`, blocking until the app shuts down.
///
/// Blocking by design: it is spawned on a dedicated thread, and holding that
/// thread is what keeps the inotify watch registered.
///
/// Failures are logged and never propagated. A missing or unreadable folder
/// must not take down the app — the folder may be on a network mount that is
/// not up yet, or the user may have changed it to something invalid. Every
/// error here degrades to "indexing happens at launch instead of live", which
/// is exactly the pre-existing behaviour.
pub fn run(dir: String) {
    let path = Path::new(&dir);
    if !path.is_dir() {
        log::warn!("watch: {dir} is not a directory, not watching");
        return;
    }

    let (tx, rx) = mpsc::channel();
    let mut watcher = match RecommendedWatcher::new(
        tx,
        Config::default().with_poll_interval(Duration::from_secs(2)),
    ) {
        Ok(w) => w,
        Err(e) => {
            log::warn!("watch: could not create watcher: {e}");
            return;
        }
    };

    // Non-recursive on purpose: the save folder is flat, and watching a whole
    // home directory would report thousands of unrelated events.
    if let Err(e) = watcher.watch(path, RecursiveMode::NonRecursive) {
        log::warn!("watch: could not watch {dir}: {e}");
        return;
    }
    log::info!("watching {dir} for new screenshots");

    let mut pending = false;
    loop {
        match rx.recv_timeout(SETTLE) {
            // Another change: keep waiting for the folder to go quiet.
            Ok(_) => pending = true,
            Err(RecvTimeoutError::Timeout) => {
                if !pending {
                    continue;
                }
                pending = false;
                reconcile(&dir);
            }
            // The watcher was dropped, which only happens at shutdown.
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Re-run the launch-time reconciliation, then wake the indexer.
///
/// `import_existing` is a `#[tauri::command]` but is a plain async fn here: it
/// is `block_on`-ed rather than driven by the Tauri runtime because this thread
/// is not part of that runtime.
///
/// `poke()` is what makes a pasted file searchable within a second instead of
/// at the next `IDLE_POLL` (up to 20 s). `import_existing` alone does not do it
/// — only `save_png` pokes, which is why a paste used to sit unindexed.
fn reconcile(dir: &str) {
    match tauri::async_runtime::block_on(crate::db::import_existing(Some(dir.to_string()))) {
        Ok(n) => {
            if n > 0 {
                log::info!("watch: imported {n} new screenshot(s)");
            }
        }
        Err(e) => log::warn!("watch: import failed: {e}"),
    }

    // `poke()` reads the `OnceLock` AppHandle rather than needing one passed in.
    crate::indexer::poke();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window has to be long enough to outlast the several separate writes
    /// a clipboard save performs, or every file triggers its own directory scan.
    #[test]
    fn settle_window_outlives_a_multistep_write() {
        assert!(SETTLE >= Duration::from_millis(200));
        // ...but short enough that a paste feels immediate rather than laggy.
        assert!(SETTLE <= Duration::from_secs(2));
    }
}
