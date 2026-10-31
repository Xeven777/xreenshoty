use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// One recognised word with its box, in image pixel coordinates.
///
/// The box is a proportional guess, not a measurement: Indra boxes whole
/// lines, and the line's width is divided between its words by character
/// count. Good enough to count words; not good enough to draw them.
#[derive(Debug, Clone, Serialize)]
pub struct Word {
    pub text: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// 0-100, the scale tesseract used. Indra reports 0-1, so it is scaled up
    /// here to keep the stored blobs comparable.
    pub confidence: f32,
}

#[derive(Debug, Default)]
pub struct OcrResult {
    /// The recognised text, one line per detected line.
    pub text: String,
    pub words: Vec<Word>,
}

/// The venv python that owns `indra_ocr` and its ONNX runtime, mirroring
/// `face.rs`'s `face-env` discovery. No fallback: a system python must not
/// silently run a different stack.
fn ocr_python() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("XREENSHOTY_OCR_PYTHON") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    dirs::data_local_dir()
        .map(|d| d.join("xreenshoty").join("ocr-env").join("bin").join("python"))
        .filter(|p| p.is_file())
}

/// The folder holding the Indra OCR models.
fn models_dir() -> PathBuf {
    std::env::var("XREENSHOTY_OCR_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("apps/models/indra-ocr")
        })
}

/// The files `indra_ocr.load` insists on. The rest (order.onnx, the other
/// recognizers, the language models) are optional and make it better, but a
/// missing one must not stop the app: `model_dir` only checks these three.
fn models_present() -> bool {
    let dir = models_dir();
    ["det.onnx", "rec.onnx", "meta.json"]
        .iter()
        .all(|f| dir.join(f).is_file())
}

fn script_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XREENSHOTY_OCR_SCRIPT_DIR") {
        return PathBuf::from(dir).join("ocr_batch.py");
    }
    // Ordered: env, then each place the script is shipped. The script lives at
    // `src-tauri/scripts/ocr/ocr_batch.py`, so:
    //   - bundled: `bundle.resources` copies `scripts/ocr` next to the binary
    //     (Linux) or under `Resources/` (macOS layout)
    //   - dev: exe at `src-tauri/target/debug/` -> `../../scripts/ocr`
    //   - then the compile-time manifest dir and cwd, so a dev run from any
    //     directory still finds it.
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("Resources/scripts/ocr"));
            candidates.push(dir.join("scripts/ocr"));
            candidates.push(dir.join("../../scripts/ocr"));
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/ocr"));
    candidates.push(PathBuf::from("src-tauri/scripts/ocr"));
    candidates.push(PathBuf::from("scripts/ocr"));
    candidates
        .iter()
        .map(|base| base.join("ocr_batch.py"))
        .find(|cand| cand.is_file())
        .unwrap_or_else(|| PathBuf::from("src-tauri/scripts/ocr/ocr_batch.py"))
}

/// How many worker processes OCR may use.
///
/// Measured on this box (12 cores, 8 physical): one worker per core with a
/// single ORT thread each beats every multi-threaded single-process setup --
/// ORT threads oversubscribe this hybrid P/E-core CPU and make a page
/// *slower* (threads=1: 267 ms/image, threads=8: 986 ms/image). Four workers
/// measured 7.35 images/s against tesseract's 4-worker 2 images/s, at the
/// same 4 cores. Six and eight measured no faster and cost more memory
/// (about 430 MB a worker), so four is the default.
fn cores() -> u32 {
    std::env::var("XREENSHOTY_OCR_CORES")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(4)
}

/// How long a whole batch may take before it is killed.
///
/// A batch of 25 screenshots measures about 5 s, so this is a watchdog against
/// a wedged interpreter, not a performance knob. Deliberately generous: a
/// killed batch loses every screenshot's text in it.
fn timeout() -> Duration {
    Duration::from_secs(
        std::env::var("XREENSHOTY_OCR_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300),
    )
}

/// True when OCR can run at all: the interpreter, the script and the models.
///
/// The models are checked here rather than inside the script because
/// `indra_ocr.model_dir` raises on a missing file *only* when a path is
/// given -- otherwise it would quietly reach for Hugging Face, and this app
/// must never touch the network at runtime.
pub fn env_ready() -> bool {
    ocr_python().is_some() && script_path().is_file() && models_present()
}

#[derive(Debug, Deserialize)]
struct BatchLine {
    #[serde(rename = "box")]
    box_xyxy: [f32; 4],
    #[serde(default)]
    text: String,
    #[serde(default)]
    conf: f32,
}

#[derive(Debug, Deserialize)]
struct BatchFile {
    path: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    lines: Vec<BatchLine>,
}

#[derive(Debug, Deserialize)]
struct BatchError {
    path: String,
    #[serde(default)]
    error: String,
}

#[derive(Debug, Deserialize)]
struct BatchOutput {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    results: Vec<BatchFile>,
    #[serde(default)]
    errors: Vec<BatchError>,
    #[serde(default)]
    error: String,
}

/// Split one recognised line into word boxes.
///
/// Indra boxes a line, not a word, and its per-character positions live in
/// internals that are not part of the public API. So the line's width is
/// divided between its words in proportion to their length: a real reading
/// order and a real word count, with boxes that are a proportional guess.
/// Never draw these.
fn split_line(text: &str, box_xyxy: [f32; 4], conf: f32) -> Vec<Word> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let lengths: Vec<usize> = words.iter().map(|w| w.chars().count()).collect();
    let total: usize = lengths.iter().sum();
    if total == 0 {
        return Vec::new();
    }
    let (x0, y0, x1, y1) = (box_xyxy[0], box_xyxy[1], box_xyxy[2], box_xyxy[3]);
    let line_w = (x1 - x0).max(0.0);
    let line_h = (y1 - y0).max(0.0);
    let mut cursor = x0.max(0.0);
    let mut out = Vec::with_capacity(words.len());
    for (word, len) in words.iter().zip(lengths) {
        let w = line_w * len as f32 / total as f32;
        out.push(Word {
            text: (*word).to_string(),
            x: cursor.round() as u32,
            y: y0.max(0.0).round() as u32,
            w: w.round() as u32,
            h: line_h.round() as u32,
            confidence: (conf * 100.0).clamp(0.0, 100.0),
        });
        cursor += w;
    }
    out
}

/// Turn one page of the batch output into text plus word boxes.
fn to_result(file: &BatchFile) -> OcrResult {
    let text = if file.text.is_empty() {
        file.lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        file.text.clone()
    };
    let words = file
        .lines
        .iter()
        .flat_map(|l| split_line(&l.text, l.box_xyxy, l.conf))
        .collect();
    OcrResult { text, words }
}

/// Parse the batch document.
///
/// Anything else that printed on stdout would break a whole-buffer parse, so
/// the last line that parses as JSON is tried as well -- the script prints one
/// document, and it is always the last thing written.
fn parse_batch_output(stdout: &[u8]) -> Result<BatchOutput, String> {
    let text = String::from_utf8_lossy(stdout);
    match serde_json::from_str::<BatchOutput>(text.trim()) {
        Ok(parsed) => Ok(parsed),
        Err(first) => {
            for line in text.lines().rev() {
                let line = line.trim();
                if line.starts_with('{') {
                    if let Ok(parsed) = serde_json::from_str::<BatchOutput>(line) {
                        return Ok(parsed);
                    }
                }
            }
            Err(format!("bad OCR json: {first}"))
        }
    }
}

/// OCR a batch of image paths.
///
/// Returns one entry per input path, in input order: `Ok` for a file that was
/// read, `Err` for a file that could not be. A failure of the batch itself --
/// no interpreter, a crashing script, malformed output -- is an `Err` on the
/// whole call, because the two cases need opposite handling: a bad file is
/// stamped as attempted, a broken environment must not lose the text.
pub fn run_batch(paths: &[String]) -> Result<Vec<Result<OcrResult, String>>, String> {
    let python = ocr_python().ok_or_else(|| {
        "ocr-env not installed: run scripts/setup-ocr.sh to create it".to_string()
    })?;
    let script = script_path();
    if !script.is_file() {
        return Err(format!("ocr script missing: {}", script.display()));
    }
    if !models_present() {
        return Err(format!(
            "OCR models missing: {}",
            models_dir().display()
        ));
    }
    let stdin = serde_json::to_string(paths).map_err(|e| format!("ocr stdin: {e}"))?;

    let mut cmd = Command::new(&python);
    cmd.arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // The models are on disk: this process must never reach the network.
        .env("INDRA_OCR_MODEL_DIR", models_dir())
        .env("HF_HUB_OFFLINE", "1")
        .env("XREENSHOTY_OCR_MODELS", models_dir())
        .env("XREENSHOTY_OCR_CORES", cores().to_string());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{} failed to start: {e}", python.display()))?;
    use std::io::Read as _;
    use std::io::Write as _;
    if let Some(mut pipe) = child.stdin.take() {
        let _ = pipe.write_all(stdin.as_bytes());
    }
    // Drain both pipes on threads while we poll. A Linux pipe holds only
    // 64 KB and a batch of 25 dense screenshots writes more than that to
    // stdout alone, so reading only after the child exits deadlocks it
    // mid-write. The classic subprocess trap.
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

    let deadline = std::time::Instant::now() + timeout();
    loop {
        match child.try_wait().map_err(|e| format!("ocr wait failed: {e}"))? {
            Some(_) => break,
            None => {}
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "OCR batch exceeded {}s and was killed",
                timeout().as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let status = child.wait().map_err(|e| format!("ocr wait failed: {e}"))?;
    let stdout = out_thread.join().unwrap_or_default();
    let stderr_bytes = err_thread.join().unwrap_or_default();

    if !status.success() {
        let err = String::from_utf8_lossy(&stderr_bytes);
        if err.contains("No module named") {
            return Err(format!(
                "ocr-env incomplete (pip install indra-ocr): {}",
                err.lines().next().unwrap_or("").trim()
            ));
        }
        return Err(format!(
            "OCR batch failed: {}",
            err.lines().next().unwrap_or("").trim()
        ));
    }
    let parsed = parse_batch_output(&stdout)?;
    if !parsed.ok {
        return Err(if parsed.error.is_empty() {
            "OCR batch reported failure".to_string()
        } else {
            parsed.error
        });
    }

    let mut results: std::collections::HashMap<&str, &BatchFile> =
        parsed.results.iter().map(|r| (r.path.as_str(), r)).collect();
    let mut errors: std::collections::HashMap<&str, &str> =
        parsed.errors.iter().map(|e| (e.path.as_str(), e.error.as_str())).collect();

    Ok(paths
        .iter()
        .map(|p| {
            if let Some(file) = results.remove(p.as_str()) {
                return Ok(to_result(file));
            }
            if let Some(err) = errors.remove(p.as_str()) {
                return Err(err.to_string());
            }
            Err("no OCR result for this file".to_string())
        })
        .collect())
}

/// Readiness probe for the UI, so Settings can name the missing piece instead
/// of showing a bare on/off.
#[derive(serde::Serialize)]
pub struct OcrStatus {
    pub engine: &'static str,
    pub venv_ready: bool,
    pub models_present: bool,
}

fn ocr_status_snapshot() -> OcrStatus {
    OcrStatus {
        engine: "Indra OCR",
        venv_ready: ocr_python().is_some() && script_path().is_file(),
        models_present: models_present(),
    }
}

#[tauri::command]
pub async fn ocr_status() -> Result<OcrStatus, String> {
    Ok(ocr_status_snapshot())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_split_into_proportional_word_boxes() {
        let words = split_line("File Edit View", [100.0, 20.0, 280.0, 40.0], 0.9);
        assert_eq!(words.len(), 3);
        assert_eq!(words[0].text, "File");
        assert_eq!((words[0].x, words[0].w), (100, 60));
        assert_eq!((words[1].x, words[1].w), (160, 60));
        assert_eq!((words[2].x, words[2].w), (220, 60));
        assert_eq!(words[0].h, 20);
        // Indra reports 0-1; the stored scale stays 0-100.
        assert!((words[0].confidence - 90.0).abs() < 0.01);
    }

    #[test]
    fn a_line_with_no_words_yields_no_boxes() {
        assert!(split_line("   ", [0.0, 0.0, 10.0, 10.0], 0.5).is_empty());
        assert!(split_line("", [0.0, 0.0, 10.0, 10.0], 0.5).is_empty());
    }

    /// CJK text has no spaces, so a line is one "word" and gets the whole box.
    #[test]
    fn a_line_without_spaces_is_one_word() {
        let words = split_line("日本語", [10.0, 5.0, 70.0, 25.0], 0.5);
        assert_eq!(words.len(), 1);
        assert_eq!((words[0].x, words[0].w), (10, 60));
    }

    #[test]
    fn the_batch_document_parses() {
        let raw = br#"{"ok": true,
            "results": [{"path": "/a.png", "text": "hello\nworld",
                         "lines": [{"box": [1,2,30,12], "text": "hello", "conf": 0.8},
                                   {"box": [1,14,40,24], "text": "world", "conf": 0.7}]}],
            "errors": []}"#;
        let parsed = parse_batch_output(raw).unwrap();
        assert!(parsed.ok);
        assert_eq!(parsed.results.len(), 1);
        let result = to_result(&parsed.results[0]);
        assert_eq!(result.text, "hello\nworld");
        assert_eq!(result.words.len(), 2);
        assert_eq!(result.words[0].text, "hello");
    }

    /// The script prints one document, but anything else on stdout must not
    /// take the whole batch down.
    #[test]
    fn stdout_noise_does_not_break_the_parse() {
        let raw = b"some warning\n{\"ok\": true, \"results\": [], \"errors\": []}\n";
        let parsed = parse_batch_output(raw).unwrap();
        assert!(parsed.ok);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(parse_batch_output(b"not json at all").is_err());
        assert!(parse_batch_output(b"").is_err());
    }

    #[test]
    fn the_script_is_found_from_any_cwd() {
        let p = script_path();
        assert!(
            p.ends_with("scripts/ocr/ocr_batch.py"),
            "unexpected script path: {}",
            p.display()
        );
        assert!(p.is_file(), "script not found at {}", p.display());
    }

    /// The status the Settings panel shows. It must never claim readiness it
    /// does not have: a false "ready" would leave new screenshots without text
    /// and nothing would say why.
    #[test]
    fn the_status_only_claims_readiness_it_has() {
        let status = ocr_status_snapshot();
        assert_eq!(status.engine, "Indra OCR");
        assert_eq!(
            status.venv_ready,
            ocr_python().is_some() && script_path().is_file()
        );
        assert_eq!(status.models_present, models_present());
        if status.venv_ready && status.models_present {
            assert!(env_ready(), "the two must agree");
        }
    }

    #[test]
    fn cores_default_and_override() {
        unsafe {
            std::env::remove_var("XREENSHOTY_OCR_CORES");
        }
        assert_eq!(cores(), 4);
        unsafe {
            std::env::set_var("XREENSHOTY_OCR_CORES", "2");
        }
        assert_eq!(cores(), 2);
        unsafe {
            std::env::set_var("XREENSHOTY_OCR_CORES", "0");
        }
        assert_eq!(cores(), 4, "zero would mean no workers");
        unsafe {
            std::env::remove_var("XREENSHOTY_OCR_CORES");
        }
    }

    /// Full pipeline against real screenshots: ocr-env -> Indra OCR -> text
    /// plus word boxes. Ignored by default because it needs the venv and the
    /// models; run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "needs the ocr-env venv and the Indra models on this machine"]
    fn batch_end_to_end_on_real_screenshots() {
        let home = dirs::home_dir().expect("home");
        let pictures = home.join("Pictures");
        let mut files: Vec<String> = std::fs::read_dir(&pictures)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        p.extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|e| e.eq_ignore_ascii_case("png"))
                    })
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        if files.is_empty() {
            eprintln!("skipping: no PNGs in {}", pictures.display());
            return;
        }
        let files = &files[..files.len().min(12)];

        let results = run_batch(files).expect("OCR batch failed");
        assert_eq!(results.len(), files.len());
        let readable = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(readable, files.len(), "every screenshot must be readable");

        // At least one of a dozen real screenshots must yield text, and the
        // boxes must line up with it.
        let mut with_text = 0;
        for outcome in &results {
            let Ok(result) = outcome else { continue };
            if result.text.trim().is_empty() {
                continue;
            }
            with_text += 1;
            assert!(
                !result.words.is_empty(),
                "text without word boxes: {:?}",
                result.text
            );
            assert!(
                result.words.iter().all(|w| w.h > 0),
                "a word box with no height"
            );
        }
        assert!(
            with_text > 0,
            "no text read from {} real screenshots",
            files.len()
        );

        // A missing file must degrade to an error for that file, not abort the
        // batch -- the indexer stamps those as attempted.
        let mixed = run_batch(&["/nonexistent/zzz.png".into()]).expect("batch");
        assert!(mixed[0].is_err(), "a missing file must be an error, not text");
    }
}
