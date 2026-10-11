use tauri::webview::PageLoadEvent;
use tauri_plugin_log::{Target, TargetKind};
use tauri_plugin_opener::OpenerExt;

pub mod db;
mod embedder;
mod indexer;
mod ocr;
mod region;
mod screenshot;

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
pub fn run() {
    tauri::Builder::default()
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            screenshot::list_monitors,
            screenshot::default_save_dir,
            screenshot::capture_fullscreen,
            screenshot::capture_region,
            screenshot::capture_active_window,
            screenshot::open_file,
            screenshot::open_folder,
            screenshot::thumbnail,
            region::begin_region_select,
            region::close_region_overlay,
            region::restore_main_window,
            region::backdrop_data_url,
            region::save_region_from_backdrop,
            db::list_recent,
            db::import_existing,
            db::search_similar,
            indexer::index_pause,
            indexer::index_status,
            db::search_text,
            db::screenshot_count
        ])
        .on_page_load(|webview, payload| {
            if webview.label() == "main" && matches!(payload.event(), PageLoadEvent::Finished) {
                log::info!("main webview finished loading");
                let _ = webview.window().show();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
