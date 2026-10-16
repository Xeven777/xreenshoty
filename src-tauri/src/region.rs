use std::sync::Mutex;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use screenshots::image::RgbaImage;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder, Window};

use crate::screenshot::{
    ensure_dir, offload, resolve_dir, save_png, screens_for_pointer,
};

pub const OVERLAY_LABEL: &str = "region-overlay";

/// Frozen grab of the monitor, shown inside the overlay so the user selects on
/// a static image. This sidesteps needing a transparent window (XFCE here has
/// no running compositor, so per-pixel alpha does not render) and guarantees the
/// saved pixels match exactly what was on screen when selection started.
#[derive(Default)]
pub struct Backdrop(pub Mutex<Option<RgbaImage>>);

fn png_data_url(image: &RgbaImage) -> Result<String, String> {
    let mut buf = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut buf, screenshots::image::ImageFormat::Png)
        .map_err(|e| format!("backdrop encode failed: {e}"))?;
    let encoded = STANDARD.encode(buf.into_inner());
    Ok(format!("data:image/png;base64,{encoded}"))
}

/// PNG data URL of the frozen backdrop, fetched by the overlay on mount rather
/// than pushed via an event, which a late listener would miss.
#[tauri::command]
pub async fn backdrop_data_url(state: State<'_, Backdrop>) -> Result<String, String> {
    // Clone under the lock, then encode on the blocking pool: the PNG encode of
    // a full-monitor frame (8 MB at 1080p, 33 MB at 4K) must not run on the main
    // thread while holding the backdrop mutex (BM1).
    let image = {
        let guard = state.0.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .ok_or_else(|| "No backdrop captured".to_string())?
            .clone()
    };
    offload(move || png_data_url(&image)).await
}

/// Bounds plus a full grab of the monitor under the cursor. Returns both from a
/// single pointer query so the overlay can never end up sized for one monitor
/// while showing the pixels of another.
pub fn capture_monitor_under_pointer() -> Result<((i32, i32, u32, u32), RgbaImage), String> {
    let bounds = screens_for_pointer()?;
    let screens = screenshots::Screen::all().map_err(|e| format!("Screen::all failed: {e}"))?;
    let target = screens
        .iter()
        .find(|s| {
            (
                s.display_info.x as i32,
                s.display_info.y as i32,
                s.display_info.width as u32,
                s.display_info.height as u32,
            ) == bounds
        })
        .ok_or_else(|| "Monitor under pointer not found among screens".to_string())?;
    let image = target
        .capture()
        .map_err(|e| format!("backdrop capture failed: {e}"))?;
    Ok((bounds, image))
}

#[tauri::command]
pub async fn begin_region_select(
    app: AppHandle,
    window: Window,
    state: State<'_, Backdrop>,
) -> Result<(), String> {
    if let Some(existing) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = existing.close();
    }

    // Hide ourselves for the whole selection, not just the grab. `always_on_top`
    // is not honoured reliably without a compositor, so showing this light-themed
    // window would cover the overlay. The backdrop is a frozen grab, so nothing
    // needs the window visible until it closes.
    let started = std::time::Instant::now();
    let _ = window.hide();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // The grab is a full-monitor capture; run it on the blocking pool so the
    // window hide/show and the async runtime are never blocked by it (BM1).
    let ((x, y, w, h), image) = match offload(capture_monitor_under_pointer).await {
        Ok(v) => v,
        Err(e) => {
            let _ = window.show();
            return Err(e);
        }
    };
    let grabbed_at = started.elapsed();
    *state.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(image);

    WebviewWindowBuilder::new(
        &app,
        OVERLAY_LABEL,
        WebviewUrl::App("region.html".into()),
    )
    .title("xreenshoty region")
    .inner_size(w as f64, h as f64)
    .position(x as f64, y as f64)
    .decorations(false)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .focused(true)
    .build()
    .map_err(|e| {
        let _ = window.show();
        *state.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
        format!("Could not open region overlay: {e}")
    })?;

    log::info!(
        "region timings: grab {}ms, window build {}ms",
        grabbed_at.as_millis(),
        started.elapsed().as_millis() - grabbed_at.as_millis()
    );
    Ok(())
}

/// Crops the selected rectangle out of the frozen backdrop and saves it. No
/// second screen grab happens, so the result is exactly what the user selected.
#[tauri::command]
pub async fn save_region_from_backdrop(
    state: State<'_, Backdrop>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    save_dir: Option<String>,
) -> Result<String, String> {
    if width == 0 || height == 0 {
        return Err("Region width and height must be > 0".to_string());
    }
    let dir = resolve_dir(save_dir)?;
    ensure_dir(&dir)?;

    // Crop while holding the lock, then release before encoding: the crop is a
    // bounded copy, the PNG encode (inside `save_png`) is the expensive half and
    // belongs on the blocking pool.
    let cropped = {
        let guard = state.0.lock().unwrap_or_else(|e| e.into_inner());
        let backdrop = guard
            .as_ref()
            .ok_or_else(|| "Backdrop is gone; region selection expired".to_string())?;

        let max_x = backdrop.width().saturating_sub(1);
        let max_y = backdrop.height().saturating_sub(1);
        if x < 0 || y < 0 || x as u32 > max_x || y as u32 > max_y {
            return Err(format!("Selection ({x}, {y}) is outside the captured area"));
        }
        let w = width.min(backdrop.width() - x as u32);
        let h = height.min(backdrop.height() - y as u32);

        screenshots::image::imageops::crop_imm(backdrop, x as u32, y as u32, w, h).to_image()
    };

    let stem = format!("region-{x}x{y}-{}x{}", cropped.width(), cropped.height());
    offload(move || save_png(cropped, &dir, &stem)).await
}

#[tauri::command]
pub fn close_region_overlay(app: AppHandle) -> Result<(), String> {
    if let Some(overlay) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = overlay.close();
    }
    Ok(())
}

#[tauri::command]
pub fn restore_main_window(app: AppHandle) -> Result<(), String> {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.set_focus();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a live X11 session. Guards the two failure modes that look
    /// identical from the UI: a blank backdrop, and a backdrop whose size does
    /// not match the monitor the overlay is sized against.
    ///
    /// Asserts on mean luma, not "has non-white pixels": a fully black grab
    /// passes a non-white check, which is exactly the wrong conclusion to draw
    /// when debugging a dark desktop.
    #[test]
    fn backdrop_is_real_and_matches_monitor() {
        let ((_, _, w, h), image) = capture_monitor_under_pointer().expect("grab failed");
        assert_eq!(
            (image.width(), image.height()),
            (w, h),
            "backdrop size must match monitor bounds"
        );

        let raw = image.as_raw();
        let samples: Vec<_> = raw.chunks_exact(4).step_by(64).collect();
        let sum: u64 = samples
            .iter()
            .map(|px| (px[0] as u64 * 299 + px[1] as u64 * 587 + px[2] as u64 * 114) / 1000)
            .sum();
        let mean = sum as f64 / samples.len() as f64;
        assert!(
            mean > 1.0,
            "backdrop looks blank: mean luma {mean:.2}/255"
        );
    }
}
