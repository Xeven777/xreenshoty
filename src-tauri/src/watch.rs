//! Watch folders so files dropped in from outside the app are indexed.
//!
//! Pasting an image, dragging one in from a browser, or `cp`-ing a batch over
//! all bypass the capture path entirely, so nothing called `save_png` and
//! nothing poked the indexer. Before this, such a file stayed invisible until
//! the next launch, and a format `import_existing` skipped stayed invisible
//! forever.
//!
//! The watcher deliberately does not parse or index anything itself: it notices
//! that *something* changed, then runs the same `import_existing` +
//! `indexer::poke()` pair the launch path uses. One reconciliation path means
//! the file-watching route cannot drift from the launch route.
//!
//! # Several folders, one registry
//!
//! `sync` is the only way in. It takes the complete desired set and makes
//! reality match: watches are added for new folders and dropped for removed
//! ones. Callers never touch `WATCHERS` directly, which is what keeps "the user
//! removed a folder in Settings" and "the app started" the same code path.
//!
//! Keys are canonicalised so `/home/me/Pictures` and `/home/me/Pictures/` are
//! one watch rather than two — inotify would accept both and every event would
//! then fire the reconcile twice.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, MutexGuard, OnceLock};
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

/// Live watches, keyed by canonical path.
///
/// A `Watcher` is kept alive by being in this map: dropping one unregisters its
/// inotify watch, which is exactly how removal works.
static WATCHERS: OnceLock<Mutex<HashMap<PathBuf, RecommendedWatcher>>> = OnceLock::new();

fn registry() -> MutexGuard<'static, HashMap<PathBuf, RecommendedWatcher>> {
    WATCHERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Make the watched set exactly `dirs`.
///
/// Returns how many folders are watched afterwards. Folders that do not exist
/// are skipped rather than failing the whole call: the user may have typed a
/// path that is not there yet, or pulled a removable drive, and losing the other
/// folders over it would be worse.
pub fn sync(dirs: &[String]) -> usize {
    // Canonicalise first so one folder is never watched twice under two
    // spellings (`~/Pictures` and `/home/me/Pictures` are the same directory).
    let mut wanted: Vec<(PathBuf, String)> = dirs
        .iter()
        .filter_map(|d| {
            let trimmed = d.trim();
            if trimmed.is_empty() {
                return None;
            }
            let path = PathBuf::from(trimmed);
            if !path.is_dir() {
                log::warn!("watch: {trimmed} is not a directory, not watching");
                return None;
            }
            let key = path.canonicalize().unwrap_or_else(|_| path.clone());
            Some((key, trimmed.to_string()))
        })
        .collect();
    wanted.sort_by(|a, b| a.0.cmp(&b.0));
    wanted.dedup_by(|a, b| a.0 == b.0);

    let mut active = registry();

    // Drop watches whose folder is no longer wanted. Removing from the map drops
    // the `Watcher`, which unregisters the watch and closes the channel its
    // thread is blocked on, so that thread ends on its own.
    active.retain(|key, _| wanted.iter().any(|(want, _)| want == key));

    let mut added = 0usize;
    for (key, display) in &wanted {
        if active.contains_key(key) {
            continue;
        }
        if let Some(watcher) = start_one(display) {
            active.insert(key.clone(), watcher);
            added += 1;
            log::info!("watching {display} for new screenshots");
        }
    }

    if added > 0 {
        log::info!("watch: now watching {} folder(s)", active.len());
    }

    active.len()
}

/// Folders currently watched, for tests and diagnostics.
pub fn watched() -> Vec<String> {
    registry()
        .keys()
        .map(|k| k.to_string_lossy().into_owned())
        .collect()
}

/// Register one folder. `None` on failure, which is logged and skipped so one
/// bad folder cannot stop the rest from being watched.
fn start_one(dir: &str) -> Option<RecommendedWatcher> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = match RecommendedWatcher::new(
        tx,
        Config::default().with_poll_interval(Duration::from_secs(2)),
    ) {
        Ok(w) => w,
        Err(e) => {
            log::warn!("watch: could not create watcher for {dir}: {e}");
            return None;
        }
    };

    // Non-recursive on purpose: save folders are flat, and watching a whole home
    // directory would report thousands of unrelated events.
    if let Err(e) = watcher.watch(Path::new(dir), RecursiveMode::NonRecursive) {
        log::warn!("watch: could not watch {dir}: {e}");
        return None;
    }

    let owned = dir.to_string();
    if let Err(e) = std::thread::Builder::new()
        .name(format!("watch:{owned}"))
        .spawn(move || drain(&owned, rx))
    {
        log::warn!("watch: could not spawn thread for {dir}: {e}");
        return None;
    }

    Some(watcher)
}

/// Block on one folder's events until its watcher is dropped.
///
/// Blocking by design: it is a dedicated thread, and holding that thread is what
/// keeps the inotify watch registered.
fn drain(dir: &str, rx: mpsc::Receiver<notify::Result<notify::Event>>) {
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
                reconcile(dir);
            }
            // The watcher was dropped, so this folder is no longer wanted.
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

/// Set the watched folders. Called by Settings whenever the list changes.
///
/// Every folder is imported immediately, not just newly added ones: a folder
/// that was unwatchable at startup (an unmounted drive) can become valid later,
/// and re-importing is idempotent via the UNIQUE `path` column.
///
/// Import runs before `sync` so a folder that cannot be read is reported by
/// `import_existing` while the watch is still being set up.
#[tauri::command]
pub async fn set_scan_folders(dirs: Vec<String>) -> Result<usize, String> {
    for dir in &dirs {
        let trimmed = dir.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Err(e) = crate::db::import_existing(Some(trimmed.to_string())).await {
            log::warn!("watch: initial import of {trimmed} failed: {e}");
        }
    }
    crate::indexer::poke();
    Ok(sync(&dirs))
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

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xreenshoty-watch-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Serialised because the registry is process-global: two tests syncing it at
    /// once would each see the other's folders.
    static LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn sync_adds_folders_and_drops_removed_ones() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let a = tmpdir("a");
        let b = tmpdir("b");

        let a_str = a.to_string_lossy().into_owned();
        let b_str = b.to_string_lossy().into_owned();

        // Start from a known-empty set, not whatever an earlier test left.
        assert_eq!(sync(&[]), 0);

        assert_eq!(sync(&[a_str.clone(), b_str.clone()]), 2);
        assert_eq!(watched().len(), 2);

        // A trailing slash and a `.` segment are the same directory, and inotify
        // would happily take both — so this is the duplicate-watch regression.
        let messy = format!("{}/./", a_str);
        assert_eq!(sync(&[a_str.clone(), messy]), 1, "must not double-watch");
        assert_eq!(watched().len(), 1);

        // Removing the only folder left must leave nothing registered.
        assert_eq!(sync(&[]), 0);
        assert!(watched().is_empty());

        std::fs::remove_dir_all(&a).ok();
        std::fs::remove_dir_all(&b).ok();
    }

    #[test]
    fn sync_skips_folders_that_do_not_exist() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let real = tmpdir("real");
        let missing = std::env::temp_dir().join("xreenshoty-watch-does-not-exist");

        let real_str = real.to_string_lossy().into_owned();
        let missing_str = missing.to_string_lossy().into_owned();

        assert_eq!(sync(&[]), 0);
        // The bad entry must not cost us the good one.
        assert_eq!(sync(&[missing_str.clone(), real_str.clone()]), 1);
        assert!(!watched().iter().any(|w| w.contains("does-not-exist")));

        // Empty strings are skipped rather than canonicalising to the CWD, which
        // would watch the whole working directory.
        assert_eq!(sync(&[String::new(), String::from("   "), real_str.clone()]), 1);

        assert_eq!(sync(&[]), 0);
        std::fs::remove_dir_all(&real).ok();
    }
}
