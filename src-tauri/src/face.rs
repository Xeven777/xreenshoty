use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// SFace embedding width. Separate from `db::EMBED_DIM` (768): the two vector
/// spaces cannot share a table, a KNN query, or a similarity score.
pub const FACE_DIM: usize = 128;
/// Provenance stamp, so a later model swap re-extracts instead of silently
/// mixing two incompatible vector spaces (same rule as `db::set_embedding`).
pub const FACE_MODEL: &str = "sface-2021dec";
/// Faces below this side length embed into near-degenerate vectors that cluster
/// together regardless of identity (spike 7.0). They are held out of the index
/// as unassigned, never as a junk blob.
pub fn min_side_px() -> f32 {
    std::env::var("XREENSHOTY_FACE_MIN_PX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(80.0)
}
/// How long the whole batch may take before it is killed. Generous on purpose:
/// ~0.1 s/image measured, so this is a watchdog against a wedged interpreter,
/// not a performance knob (same reasoning as `ocr::timeout`).
fn batch_timeout() -> Duration {
    Duration::from_secs(
        std::env::var("XREENSHOTY_FACE_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaceHit {
    pub box_xywh: [f32; 4],
    pub score: f32,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct BatchFile {
    path: String,
    faces: Vec<BatchFace>,
}

#[derive(Debug, Deserialize)]
struct BatchFace {
    #[serde(rename = "box")]
    box_xywh: [f32; 4],
    score: f32,
    embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct BatchError {
    path: String,
    error: String,
}

#[derive(Debug, Deserialize)]
struct BatchOutput {
    ok: bool,
    #[serde(default)]
    results: Vec<BatchFile>,
    #[serde(default)]
    errors: Vec<BatchError>,
    #[serde(default)]
    error: String,
}

/// The venv python that owns `onnxruntime` + `cv2`, mirroring `voice.rs`'
/// `voice-env` discovery. No fallback: system python must not silently run a
/// different stack.
fn face_python() -> Option<PathBuf> {
    dirs::data_local_dir()
        .map(|d| d.join("xreenshoty").join("face-env").join("bin").join("python"))
        .filter(|p| p.is_file())
}

fn script_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XREENSHOTY_FACE_SCRIPT_DIR") {
        return PathBuf::from(dir).join("face_batch.py");
    }
    // Ordered: env, then each place the script is shipped. The script lives at
    // `src-tauri/scripts/face/face_batch.py`, so:
    //   - bundled: `bundle.resources` copies `scripts/face` next to the binary
    //     (Linux) or under `Resources/` (macOS layout)
    //   - dev: exe at `src-tauri/target/debug/` -> `../../scripts/face`
    //   - then the compile-time manifest dir and cwd, so a dev run from any
    //     directory still finds it.
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("Resources/scripts/face"));
            candidates.push(dir.join("scripts/face"));
            candidates.push(dir.join("../../scripts/face"));
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/face"));
    candidates.push(PathBuf::from("src-tauri/scripts/face"));
    candidates.push(PathBuf::from("scripts/face"));
    candidates
        .iter()
        .map(|base| base.join("face_batch.py"))
        .find(|cand| cand.is_file())
        .unwrap_or_else(|| PathBuf::from("src-tauri/scripts/face/face_batch.py"))
}

/// Extract faces for a batch of image paths. Returns per-file hits in input
/// order; a file that fails (missing, undecodable) yields an empty hit list
/// and a log line, never an abort.
///
/// A missing venv, missing models, or malformed output is a *batch* error: the
/// caller records the attempt without scheduling a retry, so a broken
/// environment cannot spin the worker forever.
pub fn extract(paths: &[String]) -> Result<Vec<Vec<FaceHit>>, String> {
    let python = face_python().ok_or_else(|| {
        "face-env not installed: create ~/.local/share/xreenshoty/face-env with onnxruntime, opencv-python-headless, numpy, pillow"
            .to_string()
    })?;
    let script = script_path();
    if !script.is_file() {
        return Err(format!("face script missing: {}", script.display()));
    }
    let stdin = serde_json::to_string(paths).map_err(|e| format!("face stdin: {e}"))?;

    let mut cmd = Command::new(&python);
    cmd.arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("HF_HUB_OFFLINE", "1")
        .env("XREENSHOTY_FACE_MIN_PX", min_side_px().to_string());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{} failed to start: {e}", python.display()))?;
    use std::io::Read as _;
    use std::io::Write as _;
    if let Some(mut pipe) = child.stdin.take() {
        let _ = pipe.write_all(stdin.as_bytes());
    }
    // Drain both pipes on threads while we poll. A Linux pipe holds only 64 KB:
    // SFace's ONNX graph warnings fill stderr alone (~50 KB per session), and a
    // batch of many faces can overflow stdout, so reading only after exit
    // deadlocks the child mid-write. The classic subprocess trap.
    let mut stdout_pipe = child.stdout.take().expect("stdout piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr piped");
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    let deadline = std::time::Instant::now() + batch_timeout();
    loop {
        match child.try_wait().map_err(|e| format!("face wait failed: {e}"))? {
            Some(_) => break,
            None => {}
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "face batch exceeded {}s and was killed",
                batch_timeout().as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let status = child.wait().map_err(|e| format!("face wait failed: {e}"))?;
    let stdout = out_thread.join().unwrap_or_default();
    let stderr_bytes = err_thread.join().unwrap_or_default();

    if !status.success() {
        let err = String::from_utf8_lossy(&stderr_bytes);
        if err.contains("No module named") {
            return Err(format!(
                "face-env incomplete (pip install opencv-python-headless onnxruntime numpy pillow): {}",
                err.lines().next().unwrap_or("").trim()
            ));
        }
        return Err(format!(
            "face batch failed: {}",
            err.lines().next().unwrap_or("").trim()
        ));
    }
    let parsed: BatchOutput = serde_json::from_slice(&stdout)
        .map_err(|e| format!("bad face json: {e}"))?;
    if !parsed.ok {
        return Err(if parsed.error.is_empty() {
            "face batch reported failure".to_string()
        } else {
            parsed.error
        });
    }
    for be in &parsed.errors {
        log::warn!("face: {} skipped: {}", be.path, be.error);
    }
    let by_path: std::collections::HashMap<&str, &BatchFile> =
        parsed.results.iter().map(|r| (r.path.as_str(), r)).collect();
    let gate = min_side_px();
    Ok(paths
        .iter()
        .map(|p| {
            by_path
                .get(p.as_str())
                .map(|r| {
                    r.faces
                        .iter()
                        .filter(|f| {
                            f.embedding.len() == FACE_DIM
                                && f.box_xywh[2].min(f.box_xywh[3]) >= gate
                        })
                        .map(|f| FaceHit {
                            box_xywh: f.box_xywh,
                            score: f.score,
                            embedding: f.embedding.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect())
}

/// True when the interpreter and the bundled script both exist. Models are
/// checked by the script per batch, so a half-downloaded file reports cleanly
/// there instead of here.
pub fn venv_ready() -> bool {
    face_python().is_some() && script_path().is_file()
}

pub fn models_present() -> bool {
    let dir = std::env::var("XREENSHOTY_FACE_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("apps/models")
        });
    dir.join("face_detection_yunet_2023mar.onnx").is_file()
        && dir.join("face_recognition_sface_2021dec.onnx").is_file()
}

/// Readiness probe for the UI: venv + script + models, so Settings can say
/// exactly which piece is missing instead of a bare on/off. The counts are what
/// the People view's progress bar and summary are built from.
#[derive(serde::Serialize)]
pub struct FaceStatus {
    pub venv_ready: bool,
    pub models_present: bool,
    pub face_count: i64,
    /// Faces sitting in a cluster. Below `face_count`: the difference is faces
    /// seen once, or too small to index, which are deliberately ungrouped.
    pub clustered_faces: i64,
    /// People currently listed.
    pub cluster_count: i64,
    /// Screenshots whose face pass has finished. Keyed off `face_indexed_at`,
    /// which records "attempted" the same way `ocr_boxes` does, so a
    /// face-free screenshot counts as done rather than pending forever.
    pub screenshots_indexed: i64,
    pub screenshots_total: i64,
}

#[tauri::command]
pub async fn face_status() -> Result<FaceStatus, String> {
    crate::screenshot::offload(face_status_blocking).await
}

fn face_status_blocking() -> Result<FaceStatus, String> {
    let progress = crate::db::face_progress()?;
    Ok(FaceStatus {
        venv_ready: venv_ready(),
        models_present: models_present(),
        face_count: progress.face_count,
        clustered_faces: progress.clustered_faces,
        cluster_count: progress.cluster_count,
        screenshots_indexed: progress.screenshots_indexed,
        screenshots_total: progress.screenshots_total,
    })
}

/// Manual re-run from Settings: reclusters every face at the current eps.
/// Fast enough to run inline (millisecond-scale).
#[tauri::command]
pub async fn recluster_faces() -> Result<usize, String> {
    crate::screenshot::offload(crate::db::recluster_faces).await
}

/// Cluster list for the People view, largest first.
#[tauri::command]
pub async fn face_clusters() -> Result<Vec<crate::db::FaceCluster>, String> {
    crate::screenshot::offload(crate::db::face_clusters).await
}

/// Give a cluster a user-visible name (M7.5). An empty label clears it.
#[tauri::command]
pub async fn rename_cluster(cluster_id: i64, label: Option<String>) -> Result<(), String> {
    crate::screenshot::offload(move || crate::db::rename_cluster(cluster_id, label)).await
}

/// Merge `from` into `into`. Durable: recorded as a same-person link that every
/// later recluster re-applies (M7.5).
#[tauri::command]
pub async fn merge_clusters(from: i64, into: i64) -> Result<usize, String> {
    log::info!("face: merge_clusters from={from} into={into}");
    let moved = crate::screenshot::offload(move || crate::db::merge_clusters(from, into)).await?;
    log::info!("face: merged {moved} face(s) from {from} into {into}");
    Ok(moved)
}

/// Forget a person: remove their faces from the index. The screenshots stay,
/// and so does their `face_indexed_at` stamp, so the deleted person is not
/// re-detected on the next pass.
#[tauri::command]
pub async fn forget_cluster(cluster_id: i64) -> Result<usize, String> {
    log::info!("face: forget_cluster {cluster_id}");
    let removed =
        crate::screenshot::offload(move || crate::db::forget_cluster(cluster_id)).await?;
    log::info!("face: forgot {removed} face(s) from cluster {cluster_id}");
    Ok(removed)
}

/// One person's screenshots, newest first — the click-through grid.
#[tauri::command]
pub async fn cluster_screenshots(cluster_id: i64) -> Result<Vec<crate::db::Screenshot>, String> {
    crate::screenshot::offload(move || crate::db::cluster_screenshots(cluster_id)).await
}

/// 7.6: screenshots whose faces are closest to a reference face.
#[tauri::command]
pub async fn search_by_face(
    face_id: i64,
    limit: Option<u32>,
) -> Result<Vec<crate::db::SearchHit>, String> {
    crate::screenshot::offload(move || crate::db::search_by_face(face_id, limit)).await
}

/// Cropped face tile as a data URL, for the People grid. Cropped in Rust so it
/// works for any save folder without asset-protocol scope, exactly like
/// `thumbnail`. Cached on disk in the same directory Settings clears.
#[tauri::command]
pub async fn face_thumbnail(face_id: i64, size: Option<u32>) -> Result<String, String> {
    crate::screenshot::offload(move || face_thumbnail_blocking(face_id, size)).await
}

fn face_thumbnail_blocking(face_id: i64, size: Option<u32>) -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    let size = size.unwrap_or(160).clamp(32, 512);
    let (path, [x, y, w, h]) = crate::db::face_box(face_id)?;
    let src = PathBuf::from(&path);

    let cache = crate::screenshot::face_cache_file(&src, &[x, y, w, h], size);
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

    let img = screenshots::image::open(&src).map_err(|e| format!("Open failed: {e}"))?;
    let (cx, cy, cw, ch) = crop_rect(img.width(), img.height(), [x, y, w, h]);
    let tile = img.crop_imm(cx, cy, cw, ch).thumbnail(size, size);

    let mut buf = std::io::Cursor::new(Vec::new());
    tile.write_to(&mut buf, screenshots::image::ImageFormat::Png)
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

/// Crop rectangle for a face tile: the detector box padded ~15% so the tile
/// frames the head instead of cropping to the jaw, clamped inside the image so
/// a face at an edge — or a box that runs slightly off-screen — cannot overrun
/// it. Pure, so the geometry is testable without decoding or caching anything.
fn crop_rect(
    img_w: u32,
    img_h: u32,
    [x, y, w, h]: [f32; 4],
) -> (u32, u32, u32, u32) {
    let pad_x = w * 0.15;
    let pad_y = h * 0.15;
    let cx = (x - pad_x).max(0.0);
    let cy = (y - pad_y).max(0.0);
    let cw = (w + 2.0 * pad_x).min(img_w as f32 - cx).max(1.0);
    let ch = (h + 2.0 * pad_y).min(img_h as f32 - cy).max(1.0);
    (cx as u32, cy as u32, cw as u32, ch as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_defaults_and_overrides() {
        unsafe {
            std::env::remove_var("XREENSHOTY_FACE_MIN_PX");
        }
        assert_eq!(min_side_px(), 80.0);
        unsafe {
            std::env::set_var("XREENSHOTY_FACE_MIN_PX", "64");
        }
        assert_eq!(min_side_px(), 64.0);
        unsafe {
            std::env::set_var("XREENSHOTY_FACE_MIN_PX", "junk");
        }
        assert_eq!(min_side_px(), 80.0);
        unsafe {
            std::env::remove_var("XREENSHOTY_FACE_MIN_PX");
        }
    }

    /// The crop is padded, and — the part that actually bites — always stays
    /// inside the image. A detector box can run off an edge, and `crop_imm`
    /// panics rather than clipping, which would take down a People grid over
    /// one face in a corner.
    #[test]
    fn crop_rect_pads_and_stays_inside_the_image() {
        let (x, y, w, h) = crop_rect(1000, 1000, [400.0, 400.0, 100.0, 100.0]);
        assert_eq!((x, y, w, h), (385, 385, 130, 130), "padded by 15%");

        // Bottom-right corner: the padded box must not overrun the image.
        let (x, y, w, h) = crop_rect(200, 150, [180.0, 140.0, 40.0, 40.0]);
        assert!(x + w <= 200, "{x}+{w} overflows 200");
        assert!(y + h <= 150, "{y}+{h} overflows 150");
        assert!(w > 0 && h > 0, "crop must keep a non-zero area");

        // Box partly off-screen: origin clamps to 0, size never goes negative.
        let (x, y, w, h) = crop_rect(200, 150, [-30.0, -10.0, 60.0, 60.0]);
        assert_eq!((x, y), (0, 0), "origin must clamp to the image");
        assert!(w > 0 && h > 0);

        // Degenerate box still yields a drawable rect.
        let (_, _, w, h) = crop_rect(200, 150, [10.0, 10.0, 0.0, 0.0]);
        assert!(w >= 1 && h >= 1, "w={w} h={h}");
    }

    #[test]
    fn batch_output_rejects_wrong_dims_and_small_boxes() {        let emb128: Vec<f32> = (0..128).map(|i| i as f32 / 128.0).collect();
        let parsed: BatchOutput = serde_json::from_value(serde_json::json!({
            "ok": true,
            "results": [
                {"path": "a.png", "faces": [
                    {"box": [0.0, 0.0, 200.0, 200.0], "score": 0.9,
                     "embedding": emb128},
                    {"box": [0.0, 0.0, 10.0, 10.0], "score": 0.9,
                     "embedding": emb128},
                    {"box": [0.0, 0.0, 200.0, 200.0], "score": 0.9,
                     "embedding": [0.1, 0.2]}
                ]}
            ],
            "errors": []
        }))
        .unwrap();
        assert!(parsed.ok);
        let gate = 80.0f32;
        let kept: Vec<&BatchFace> = parsed.results[0]
            .faces
            .iter()
            .filter(|f| {
                f.embedding.len() == FACE_DIM
                    && f.box_xywh[2].min(f.box_xywh[3]) >= gate
            })
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].score, 0.9);
    }

    #[test]
    fn dim_and_model_labels_are_stable() {
        assert_eq!(FACE_DIM, 128);
        assert_eq!(FACE_MODEL, "sface-2021dec");
    }

    /// `script_path` must find the script from any dev cwd — this is what the
    /// broken `../../` candidate would have failed.
    #[test]
    fn script_path_finds_the_bundled_script() {
        let p = script_path();
        assert!(
            p.ends_with("scripts/face/face_batch.py"),
            "unexpected script path: {}",
            p.display()
        );
        assert!(p.is_file(), "script not found at {}", p.display());
    }

    /// Full pipeline against real files: face-env venv -> YuNet -> align ->
    /// SFace -> 128-d vectors. Ignored by default because it needs the venv
    /// and ~/apps/models; run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "needs face-env + ONNX models on this machine"]
    fn extract_end_to_end_on_a_known_portrait() {
        let home = dirs::home_dir().expect("home");
        let png = home.join("Pictures/anish.png");
        if !png.is_file() {
            eprintln!("skipping: {} not present", png.display());
            return;
        }
        let paths = vec![png.to_string_lossy().into_owned()];
        let hits = extract(&paths).expect("face batch failed");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].len(), 1, "expected exactly one face in the portrait");
        assert_eq!(hits[0][0].embedding.len(), FACE_DIM);
        assert!(
            hits[0][0].box_xywh[2].min(hits[0][0].box_xywh[3]) >= min_side_px(),
            "the 80 px gate must hold inside extract()"
        );
        // A missing file must degrade to an empty hit, not abort the batch.
        let mixed = extract(&["/nonexistent/zzz.png".into()]).expect("batch");
        assert!(mixed[0].is_empty());
    }
}


