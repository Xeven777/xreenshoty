use std::process::Child;
use std::sync::Mutex;

static RECORDING: Mutex<Option<Recording>> = Mutex::new(None);

struct Recording {
    child: Child,
    path: std::path::PathBuf,
}

fn record_bin() -> (&'static str, Vec<&'static str>) {
    if std::process::Command::new("pw-record").arg("--help").output().is_ok() {
        ("pw-record", vec!["--rate", "16000", "--channels", "1"])
    } else {
        ("arecord", vec!["-f", "cd", "-t", "wav"])
    }
}

#[tauri::command]
pub async fn voice_start() -> Result<(), String> {
    let mut slot = RECORDING.lock().map_err(|e| format!("lock poisoned: {e}"))?;
    if slot.is_some() {
        return Err("already recording".into());
    }
    let path = std::env::temp_dir().join(format!("xreenshoty-voice-{}.wav", std::process::id()));
    let (bin, extra) = record_bin();
    let child = std::process::Command::new(bin)
        .args(&extra)
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("{bin} failed to start: {e}"))?;
    *slot = Some(Recording { child, path });
    Ok(())
}

#[tauri::command]
pub async fn voice_stop() -> Result<String, String> {
    let (child, path) = {
        let mut slot = RECORDING.lock().map_err(|e| format!("lock poisoned: {e}"))?;
        let rec = slot.take().ok_or("not recording")?;
        (rec.child, rec.path)
    };
    stop_gracefully(child, &path)?;
    transcribe_wav(&path)
}

/// SIGTERM first so the recorder flushes a valid WAV header; SIGKILL leaves a
/// truncated file (`fmt chunk missing`) that the transcriber rejects.
fn stop_gracefully(mut child: Child, path: &std::path::Path) -> Result<(), String> {
    // SIGTERM via `kill` CLI: Child has no signal API without extra crates.
    let _ = std::process::Command::new("kill")
        .arg(child.id().to_string())
        .output();
    // Wait up to 2 s for a clean exit before escalating.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match child.try_wait().map_err(|e| format!("wait failed: {e}"))? {
            Some(_) => break,
            None if std::time::Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            None => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
    // A <1 s tap can still leave a header-only file; reject it with a helpful
    // message instead of a wave.py traceback.
    let len = std::fs::metadata(path)
        .map(|m| m.len())
        .unwrap_or(0);
    if len < 4096 {
        let _ = std::fs::remove_file(path);
        return Err("recording too short — hold the mic while you speak".into());
    }
    Ok(())
}

fn transcribe_wav(path: &std::path::Path) -> Result<String, String> {
    let script = format!(
        "import json, needle; r = needle.transcribe({:?}); print(json.dumps(r))",
        path.to_string_lossy().into_owned()
    );
    // Prefer our venv python (has cactus-needle); fall back to system python3.
    let venv_python = dirs::data_local_dir()
        .map(|d| d.join("xreenshoty").join("voice-env").join("bin").join("python"));
    let python = venv_python
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "python3".into());
    let out = std::process::Command::new(&python)
        .args(["-c", &script])
        .env("NEEDLE_TELEMETRY", "0")
        // Weights (~17 MB) are cached in ~/.cache after first use; fail fast
        // rather than hitting the network, so voice stays fully offline.
        .env("HF_HUB_OFFLINE", "1")
        .output()
        .map_err(|e| format!("{python} failed: {e}"))?;
    let _ = std::fs::remove_file(path);
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("No module named") {
            return Err("cactus-needle not installed: pip install cactus-needle".into());
        }
        if err.contains("fmt chunk") || err.contains("wave.Error") {
            return Err("recording was cut off — hold the mic a beat longer".into());
        }
        return Err(format!("transcribe failed: {err}"));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("bad transcript json: {e}"))?;
    Ok(v.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string())
}

#[tauri::command]
pub async fn voice_cancel() -> Result<(), String> {
    let rec = RECORDING.lock().map_err(|e| format!("lock poisoned: {e}"))?.take();
    if let Some(mut rec) = rec {
        let _ = rec.child.kill();
        let _ = rec.child.wait();
        let _ = std::fs::remove_file(&rec.path);
    }
    Ok(())
}
