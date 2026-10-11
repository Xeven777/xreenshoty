use chrono::Local;
use screenshots::Screen;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct MonitorInfo {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

fn monitor_info(screen: &Screen) -> MonitorInfo {
    let di = &screen.display_info;
    MonitorInfo {
        id: di.id as u32,
        x: di.x as i32,
        y: di.y as i32,
        width: di.width as u32,
        height: di.height as u32,
    }
}

pub(crate) fn resolve_dir(save_dir: Option<String>) -> Result<PathBuf, String> {
    if let Some(dir) = save_dir {
        let trimmed = dir.trim();
        if trimmed.is_empty() {
            return Err("Save directory is empty".to_string());
        }
        Ok(PathBuf::from(trimmed))
    } else {
        dirs::picture_dir().ok_or_else(|| "Could not resolve Pictures directory".to_string())
    }
}

pub(crate) fn ensure_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Create save dir failed: {e}"))?;
    if !dir.is_dir() {
        return Err(format!("Not a directory: {}", dir.display()));
    }
    Ok(())
}

fn timestamp() -> String {
    Local::now().format("%Y-%m-%d-%H-%M-%S").to_string()
}

fn sanitize(name: &str) -> String {
    name.replace(['|', '\\', ':', '/'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

/// Capture a rectangle in virtual-desktop coordinates by routing it to the
/// monitor that contains its top-left corner.
fn capture_virtual_rect(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<screenshots::image::RgbaImage, String> {
    if width == 0 || height == 0 {
        return Err("Region width and height must be > 0".to_string());
    }
    let screens = Screen::all().map_err(|e| format!("Screen::all failed: {e}"))?;
    let screen = screens
        .iter()
        .find(|s| {
            let di = &s.display_info;
            let (sx, sy, sw, sh) = (di.x as i32, di.y as i32, di.width as i64, di.height as i64);
            (x as i64) >= (sx as i64)
                && (x as i64) < (sx as i64 + sw)
                && (y as i64) >= (sy as i64)
                && (y as i64) < (sy as i64 + sh)
        })
        .ok_or_else(|| format!("Point ({x}, {y}) is outside all monitors"))?;

    let di = &screen.display_info;
    let rel_x = x - di.x as i32;
    let rel_y = y - di.y as i32;
    // Clamp to monitor bounds so oversized regions degrade gracefully.
    let max_w = (di.width as i32 - rel_x).max(0) as u32;
    let max_h = (di.height as i32 - rel_y).max(0) as u32;
    let w = width.min(max_w);
    let h = height.min(max_h);
    if w == 0 || h == 0 {
        return Err("Region is outside monitor bounds".to_string());
    }
    screen
        .capture_area(rel_x, rel_y, w, h)
        .map_err(|e| format!("capture_area failed: {e}"))
}

pub(crate) fn save_png(image: screenshots::image::RgbaImage, dir: &Path, stem: &str) -> Result<String, String> {
    let filename = format!("xreenshoty-{}-{}.png", timestamp(), sanitize(stem));
    let path = dir.join(filename);
    let (width, height) = (image.width(), image.height());
    image
        .save(&path)
        .map_err(|e| format!("save failed for {}: {e}", path.display()))?;
    // Every capture mode funnels through here, so this is the one place that
    // needs to index. Files stay authoritative; the DB is just a searchable
    // index over them.
    crate::db::record(&path, width, height, stem);
    // Nudge the background indexer so this screenshot becomes searchable
    // immediately instead of at the next idle poll.
    crate::indexer::poke();
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub fn list_monitors() -> Result<Vec<MonitorInfo>, String> {
    let screens = Screen::all().map_err(|e| format!("Screen::all failed: {e}"))?;
    Ok(screens.iter().map(monitor_info).collect())
}

#[tauri::command]
pub fn default_save_dir() -> Result<String, String> {
    Ok(resolve_dir(None)?
        .to_string_lossy()
        .to_string())
}

#[tauri::command]
pub fn capture_fullscreen(
    monitor_id: Option<u32>,
    save_dir: Option<String>,
) -> Result<Vec<String>, String> {
    let dir = resolve_dir(save_dir)?;
    ensure_dir(&dir)?;

    let screens = Screen::all().map_err(|e| format!("Screen::all failed: {e}"))?;
    if screens.is_empty() {
        return Err("No screens found".to_string());
    }

    let targets: Vec<&Screen> = match monitor_id {
        Some(id) => {
            let one = screens
                .iter()
                .find(|s| s.display_info.id as u32 == id)
                .ok_or_else(|| format!("No monitor with id {id}"))?;
            vec![one]
        }
        None => screens.iter().collect(),
    };

    let mut saved = Vec::new();
    for screen in targets {
        let name = format!("display-{}", screen.display_info.id);
        let image = screen
            .capture()
            .map_err(|e| format!("capture failed on {name}: {e}"))?;
        saved.push(save_png(image, &dir, &name)?);
    }
    Ok(saved)
}

/// Bounds of the monitor under the mouse cursor, so the region overlay opens
/// on the display the user is actually pointing at.
#[cfg(target_os = "linux")]
pub fn screens_for_pointer() -> Result<(i32, i32, u32, u32), String> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt;

    let (conn, screen_num) =
        x11rb::connect(None).map_err(|e| format!("X11 connect failed: {e}"))?;
    let root = conn.setup().roots[screen_num].root;
    let reply = conn
        .query_pointer(root)
        .map_err(|e| format!("X11 query_pointer failed: {e}"))?
        .reply()
        .map_err(|e| format!("X11 reply failed: {e}"))?;
    let (px, py) = (reply.root_x as i32, reply.root_y as i32);

    let screens = Screen::all().map_err(|e| format!("Screen::all failed: {e}"))?;
    screens
        .iter()
        .find(|s| {
            let di = &s.display_info;
            let (sx, sy) = (di.x as i32, di.y as i32);
            let (sw, sh) = (di.width as i32, di.height as i32);
            px >= sx && px < sx + sw && py >= sy && py < sy + sh
        })
        .or_else(|| screens.first())
        .map(|s| {
            (
                s.display_info.x as i32,
                s.display_info.y as i32,
                s.display_info.width as u32,
                s.display_info.height as u32,
            )
        })
        .ok_or_else(|| "No screens found".to_string())
}

#[cfg(not(target_os = "linux"))]
pub fn screens_for_pointer() -> Result<(i32, i32, u32, u32), String> {
    let screens = Screen::all().map_err(|e| format!("Screen::all failed: {e}"))?;
    screens
        .first()
        .map(|s| {
            (
                s.display_info.x as i32,
                s.display_info.y as i32,
                s.display_info.width as u32,
                s.display_info.height as u32,
            )
        })
        .ok_or_else(|| "No screens found".to_string())
}

#[tauri::command]
pub fn capture_region(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    save_dir: Option<String>,
) -> Result<String, String> {
    let dir = resolve_dir(save_dir)?;
    ensure_dir(&dir)?;
    let image = capture_virtual_rect(x, y, width, height)?;
    save_png(image, &dir, &format!("region-{x}x{y}-{width}x{height}"))
}

/// Outer geometry (including window-manager borders) of the active window
/// in virtual-desktop coordinates.
#[cfg(target_os = "linux")]
fn active_window_rect() -> Result<(i32, i32, u32, u32), String> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, Window};

    let (conn, screen_num) = x11rb::connect(None).map_err(|e| format!("X11 connect failed: {e}"))?;
    let root = conn.setup().roots[screen_num].root;

    let net_active = conn
        .intern_atom(false, b"_NET_ACTIVE_WINDOW")
        .map_err(|e| format!("X11 request failed: {e}"))?
        .reply()
        .map_err(|e| format!("X11 reply failed: {e}"))?
        .atom;
    let net_frame = conn
        .intern_atom(false, b"_NET_FRAME_EXTENTS")
        .map_err(|e| format!("X11 request failed: {e}"))?
        .reply()
        .map_err(|e| format!("X11 reply failed: {e}"))?
        .atom;

    let active: Window = conn
        .get_property(false, root, net_active, AtomEnum::WINDOW, 0, 1)
        .map_err(|e| format!("X11 request failed: {e}"))?
        .reply()
        .map_err(|e| format!("X11 reply failed: {e}"))?
        .value32()
        .and_then(|mut v| v.next())
        .map(|id| id as Window)
        .filter(|&w| w != 0)
        .ok_or_else(|| "No active window (desktop focused?)".to_string())?;

    // Frame extents: left, right, top, bottom border sizes (may be absent).
    let (pad_l, pad_r, pad_t, pad_b) = conn
        .get_property(false, active, net_frame, AtomEnum::CARDINAL, 0, 4)
        .ok()
        .and_then(|c| c.reply().ok())
        .and_then(|r| {
            let mut v = r.value32()?;
            Some((v.next()? as i32, v.next()? as i32, v.next()? as i32, v.next()? as i32))
        })
        .unwrap_or((0, 0, 0, 0));

    let geom = conn
        .get_geometry(active)
        .map_err(|e| format!("X11 request failed: {e}"))?
        .reply()
        .map_err(|e| format!("X11 reply failed: {e}"))?;
    // Geometry is parent-relative (reparenting WMs); translate to root coords.
    let pos = conn
        .translate_coordinates(active, root, geom.x, geom.y)
        .map_err(|e| format!("X11 request failed: {e}"))?
        .reply()
        .map_err(|e| format!("X11 reply failed: {e}"))?;

    let x = pos.dst_x as i32 - pad_l;
    let y = pos.dst_y as i32 - pad_t;
    let w = (geom.width as i32 + pad_l + pad_r).max(0) as u32;
    let h = (geom.height as i32 + pad_t + pad_b).max(0) as u32;
    Ok((x, y, w, h))
}

#[cfg(not(target_os = "linux"))]
fn active_window_rect() -> Result<(i32, i32, u32, u32), String> {
    Err("Active-window capture is Linux-only in this milestone (Windows/macOS come with the xcap migration)".to_string())
}

#[tauri::command]
pub fn capture_active_window(save_dir: Option<String>) -> Result<String, String> {
    let dir = resolve_dir(save_dir)?;
    ensure_dir(&dir)?;

    let (x, y, w, h) = active_window_rect()?;
    if w == 0 || h == 0 {
        return Err("Active window has zero size (minimized?)".to_string());
    }
    let image = capture_virtual_rect(x, y, w, h)
        .map_err(|e| format!("Window at ({x}, {y} {w}x{h}) not capturable: {e}"))?;
    save_png(image, &dir, "active-window")
}

/// Hand a screenshot to the system image viewer.
///
/// Uses the same xdg-open path as `open_folder`, so whatever the user already
/// has configured for images is what they get.
#[tauri::command]
pub async fn open_file(path: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(&path);
    if !p.is_file() {
        return Err(format!("not a file: {path}"));
    }
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open")
        .arg(&p)
        .spawn()
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(&p)
        .spawn()
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer")
        .arg(&p)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn open_folder(path: Option<String>) -> Result<(), String> {
    let dir = resolve_dir(path)?;
    ensure_dir(&dir)?;
    #[cfg(target_os = "linux")]
    let r = std::process::Command::new("xdg-open")
        .arg(&dir)
        .spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(&dir).spawn();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer").arg(&dir).spawn();
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let r = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "open_folder not supported on this OS",
    ));
    r.map(|_| ()).map_err(|e| format!("Could not open folder: {e}"))?;
    Ok(())
}

/// Stable FNV-1a hash. Used only as a cache filename, so it just has to be
/// deterministic across runs — `DefaultHasher` is explicitly not.
fn cache_key(parts: &[&[u8]]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in *part {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

/// Cached thumbnails live under the OS cache dir, keyed by source path plus
/// mtime and size so an edited screenshot regenerates instead of going stale.
fn thumb_cache_file(path: &Path, size: u32) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let key = cache_key(&[
        path.to_string_lossy().as_bytes(),
        &size.to_le_bytes(),
        &mtime.to_le_bytes(),
        &meta.len().to_le_bytes(),
    ]);
    Some(
        dirs::cache_dir()?
            .join("xreenshoty")
            .join("thumbs")
            .join(format!("{key}.png")),
    )
}

/// Downscaled PNG data URL for session-history thumbnails. Served from Rust so
/// previews work for any save directory without asset-protocol scope.
///
/// Generating one means a full PNG decode, so results are cached on disk: the
/// history grid asks for the same 60 images on every launch, and re-deriving
/// them each time is pure waste. A cache miss is indistinguishable from a slow
/// decode, which is what made this worth fixing.
#[tauri::command]
pub async fn thumbnail(path: String, size: Option<u32>) -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    let size = size.unwrap_or(320).clamp(32, 1024);
    let source = PathBuf::from(&path);

    let cache = thumb_cache_file(&source, size);
    if let Some(cached) = &cache {
        if let Ok(bytes) = std::fs::read(cached) {
            if !bytes.is_empty() {
                return Ok(format!(
                    "data:image/png;base64,{}",
                    STANDARD.encode(&bytes)
                ));
            }
        }
    }

    let img = screenshots::image::open(&source).map_err(|e| format!("Open failed: {e}"))?;
    let thumb = img.thumbnail(size, size);
    let mut buf = std::io::Cursor::new(Vec::new());
    thumb
        .write_to(&mut buf, screenshots::image::ImageFormat::Png)
        .map_err(|e| format!("Encode failed: {e}"))?;
    let png = buf.into_inner();

    if let Some(cached) = &cache {
        if let Some(parent) = cached.parent() {
            let _ = std::fs::create_dir_all(parent);
            let _ = std::fs::write(cached, &png);
        }
    }

    Ok(format!("data:image/png;base64,{}", STANDARD.encode(&png)))
}

#[cfg(test)]
mod thumbnail_tests {
    use super::*;

    #[test]
    fn cache_key_is_stable_and_distinguishes_inputs() {
        // Determinism across processes matters: a key that drifts would silently
        // invalidate the whole cache on every Rust upgrade.
        assert_eq!(cache_key(&[b"/a.png", b"320"]), cache_key(&[b"/a.png", b"320"]));
        assert_ne!(cache_key(&[b"/a.png", b"320"]), cache_key(&[b"/a.png", b"640"]));
        assert_ne!(cache_key(&[b"/a.png", b"320"]), cache_key(&[b"/b.png", b"320"]));
    }

    #[tokio::test]
    async fn thumbnail_is_cached_and_regenerates_when_source_changes() {
        let dir = std::env::temp_dir().join("xreenshoty-thumb-test");
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("t.png");
        let path = png.to_string_lossy().to_string();

        screenshots::image::RgbaImage::new(900, 600).save(&png).unwrap();
        let first = thumbnail(path.clone(), Some(64)).await.unwrap();
        assert!(first.starts_with("data:image/png;base64,"));

        // Second call must be served from disk, and produce identical bytes.
        let second = thumbnail(path.clone(), Some(64)).await.unwrap();
        assert_eq!(first, second);

        // Changing the source must invalidate the entry rather than serve stale.
        screenshots::image::RgbaImage::new(32, 32).save(&png).unwrap();
        let third = thumbnail(path, Some(64)).await.unwrap();
        assert_ne!(first, third, "stale thumbnail served after source changed");

        let cache_dir = dirs::cache_dir().unwrap().join("xreenshoty").join("thumbs");
        if let Ok(entries) = std::fs::read_dir(&cache_dir) {
            for e in entries.flatten() {
                let _ = std::fs::remove_file(e.path());
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
