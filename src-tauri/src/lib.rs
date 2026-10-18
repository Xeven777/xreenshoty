use tauri::webview::PageLoadEvent;
use tauri_plugin_log::{Target, TargetKind};
use tauri_plugin_opener::OpenerExt;

pub mod db;
mod cluster;
mod embedder;
mod face;
mod indexer;
mod ocr;
mod region;
mod screenshot;
mod voice;
pub mod watch;

fn external_navigation_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::<R>::new("external-navigation")
        .on_navigation(|webview, url| {
            let is_internal_host = matches!(
                url.host_str(),
                Some("localhost") | Some("127.0.0.1") | Some("tauri.localhost") | Some("::1")
            );

            let is_internal = url.scheme() == "tauri" || is_internal_host;

            if is_internal {
                return true;
            }

            let is_external_link = matches!(url.scheme(), "http" | "https" | "mailto" | "tel");

            if is_external_link {
                log::info!("opening external link in system browser: {}", url);
                let _ = webview.opener().open_url(url.as_str(), None::<&str>);
                return false;
            }

            true
        })
        .build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Watch folders so files that appear from outside the app are indexed.
///
/// A pasted or dragged-in image never calls `save_png`, so nothing poked the
/// indexer and the file stayed invisible to search. The watcher runs
/// `import_existing` + `poke` whenever a folder goes quiet, which is the same
/// reconciliation the launch path performs.
///
/// Rust holds no settings state (`AGENTS.md`): the folder list lives in the
/// webview's `localStorage`. At startup we cannot read it, so this seeds the
/// registry with the default Pictures folder only; the frontend calls
/// `set_scan_folders` with the real list as soon as settings load, and again on
/// every change. Seeding means behaviour before that first call matches the old
/// single-folder watcher.
///
/// No thread is spawned here: each watch owns its own, started by `watch::sync`.
fn seed_folder_watch() {
    match screenshot::resolve_dir(None) {
        Ok(dir) => {
            watch::sync(&[dir.to_string_lossy().into_owned()]);
        }
        Err(e) => log::warn!("watch: no save folder to watch: {e}"),
    }
}

pub fn run() {
    let app = tauri::Builder::default()
        .manage(region::Backdrop::default())
        .plugin(
            tauri_plugin_log::Builder::new()
                // Without this, HTTP transport crates log every byte at TRACE
                // and bury the lines that actually diagnose problems — the
                // log file became 100% ureq noise.
                .level(log::LevelFilter::Info)
                .filter(|metadata| {
                    metadata.target().starts_with("tauri_native_lib")
                        || metadata.target().starts_with("xreenshoty")
                })
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::LogDir { file_name: None }),
                    Target::new(TargetKind::Webview),
                ])
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(external_navigation_plugin())
        .setup(|app| {
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_global_shortcut::Builder::new().build())?;
            indexer::Indexer::start(app.handle().clone());
            seed_folder_watch();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            screenshot::list_monitors,
            screenshot::default_save_dir,
            screenshot::capture_fullscreen,
            screenshot::capture_region,
            screenshot::capture_active_window,
            screenshot::delete_screenshot,
            screenshot::open_file,
            screenshot::open_folder,
            screenshot::thumbnail,
            screenshot::thumbnail_cache_size,
            screenshot::clear_thumbnail_cache,
            region::begin_region_select,
            region::close_region_overlay,
            region::restore_main_window,
            region::backdrop_data_url,
            region::save_region_from_backdrop,
            db::list_recent,
            db::import_existing,
            db::search_hybrid,
            db::search_similar,
            indexer::index_pause,
            indexer::index_status,
            indexer::set_face_indexing,
            db::search_text,
            db::screenshot_detail,
            db::screenshot_count,
            watch::set_scan_folders,
            voice::voice_start,
            voice::voice_stop,
            voice::voice_cancel,
            face::face_status,
            face::recluster_faces,
            face::face_clusters
        ])
        .on_page_load(|webview, payload| {
            if webview.label() == "main" && matches!(payload.event(), PageLoadEvent::Finished) {
                log::info!("main webview finished loading");
                let _ = webview.window().show();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // Kill the llama-server this process spawned so an ~825 MB model is never
    // left resident. Hooked to `RunEvent::Exit` as well as the return path,
    // because a window closed by the WM can end the process without the
    // builder's closure ever running, and an orphaned server holds the port and
    // ~825 MB of GPU/CPU memory.
    app.run(|_handle, event| {
        if let tauri::RunEvent::Exit = event {
            indexer::shutdown();
        }
    });

    indexer::shutdown();
}
