use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};

/// Historical label for the default model. Do not change it: it is the value
/// already stored against every existing `screenshot_vec` row, and a different
/// string would mark the whole library stale and trigger a full re-embed.
pub const DEFAULT_MODEL_LABEL: &str = "embeddinggemma-2-q8_0";

/// Where the model files live. Overridable so this is not hardcoded to one
/// machine: env first, then the layout used during the runtime spike.
#[derive(Debug, Clone)]
pub struct Config {
    pub server_bin: PathBuf,
    pub model: PathBuf,
    pub mmproj: PathBuf,
    pub device: String,
    pub host: String,
    pub port: u16,
    /// Vision-token budget per image. A full 1920x1080 capture expands to 924
    /// tokens and 9.3s on the Arc; 320 preserves the nearest-neighbour ranking
    /// exactly (6/6) at 1.4s. The model accepts 70-1120.
    pub image_max_tokens: u32,
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).map(PathBuf::from)
}

impl Config {
    pub fn discover() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let apps = home.join("apps");

        Self {
            server_bin: env_path("XREENSHOTY_LLAMA_SERVER")
                .unwrap_or_else(|| apps.join("llama-b11541/llama-server")),
            model: env_path("XREENSHOTY_EMBED_MODEL").unwrap_or_else(|| {
                apps.join("models/embeddinggemma-2-Q8_0.gguf")
            }),
            mmproj: env_path("XREENSHOTY_EMBED_MMPROJ").unwrap_or_else(|| {
                apps.join("models/mmproj-embeddinggemma-2-Q8_0.gguf")
            }),
            device: std::env::var("XREENSHOTY_EMBED_DEVICE")
                .unwrap_or_else(|_| resolve_device()),
            host: "127.0.0.1".into(),
            port: std::env::var("XREENSHOTY_EMBED_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8080),
            image_max_tokens: std::env::var("XREENSHOTY_IMAGE_MAX_TOKENS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(320),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}:{}{}", self.host, self.port, path)
    }

    /// Model name recorded against each vector, so a later model change can be
    /// detected instead of silently mixing incompatible embeddings (BM7).
    ///
    /// The default model keeps a fixed historical label. Deriving the label from
    /// the filename would change it for existing installs (and re-embed the whole
    /// library) with no actual model change. Any *other* model is labelled by its
    /// own file names, so pointing `XREENSHOTY_EMBED_MODEL` at a different
    /// checkpoint marks every old vector stale and forces a clean re-embed.
    fn label(&self) -> String {
        let stem = self
            .model
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        if stem == "embeddinggemma-2-Q8_0" {
            return DEFAULT_MODEL_LABEL.to_string();
        }
        let mmproj = self
            .mmproj
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if mmproj.is_empty() {
            stem.to_string()
        } else {
            format!("{stem}+{mmproj}")
        }
    }

    fn files_present(&self) -> Result<(), String> {
        for p in [&self.server_bin, &self.model, &self.mmproj] {
            if !p.is_file() {
                return Err(format!("missing: {}", p.display()));
            }
        }
        Ok(())
    }
}

/// Pick the Vulkan device index for inference.
///
/// A discrete GPU is wanted: the vision tower falls back toward software paths
/// on the iGPU (measured 17.2 s/image vs 1.2 s on the Arc, ~14x). Vulkan indices
/// are an enumeration order that varies by machine, so rather than hardcode
/// "1" we ask Vulkan what is there and prefer a device that names itself as
/// discrete. Falls back when `vulkaninfo` is missing or unhelpful.
fn resolve_device() -> String {
    match std::process::Command::new("vulkaninfo").arg("--summary").output() {
        Ok(out) => resolve_device_from(&String::from_utf8_lossy(&out.stdout)),
        Err(_) => {
            log::info!("embedding: vulkaninfo unavailable, defaulting to device 1");
            "1".to_string()
        }
    }
}

/// Split out from `resolve_device` so the parsing is testable without spawning
/// `vulkaninfo`. Line order matches ggml's enumeration, so position is the index.
fn resolve_device_from(summary: &str) -> String {
    const DISCRETE: [&str; 6] = ["arc", "nvidia", "geforce", "amd", "radeon", "apple"];

    {
        // Same enumeration order as ggml uses, so line position is the index.
        let text = summary.to_lowercase();
        let names: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("devicename"))
            .map(|l| l.split('=').nth(1).unwrap_or("").trim())
            .collect();

        // Indices are only meaningful next to their names: `DRI_PRIME=1`
        // (set by the XFCE session) makes Mesa promote the dGPU to index 0, so
        // the same machine can enumerate Arc=0 for the app and Arc=1 in a
        // plain shell. Printing the whole list keeps that verifiable at a glance.
        for (i, name) in names.iter().enumerate() {
            log::info!("embedding: vulkan device {i} = {name}");
        }
        for (i, name) in names.iter().enumerate() {
            if DISCRETE.iter().any(|d| name.contains(d)) {
                log::info!("embedding: auto-selected Vulkan device {i} ({name})");
                return i.to_string();
            }
        }
        if let Some(first) = names.first() {
            log::warn!("embedding: no discrete GPU found, using device 0 ({first})");
            return "0".to_string();
        }
    }

    log::info!("embedding: vulkaninfo reported no devices, defaulting to device 1");
    "1".to_string()
}

/// Owns the llama-server child process and talks to its HTTP endpoint.
///
/// Uses `/v1/embeddings`, because `/embedding` silently drops image parts and
/// returns a vector of the text field alone.
pub struct Embedder {
    config: Config,
    child: Mutex<Option<Child>>,
    /// Health checks must fail fast or the worker stalls; embedding calls get a
    /// generous ceiling because a full-screen screenshot takes ~0.4s.
    health: ureq::Agent,
    embed: ureq::Agent,
}

impl Embedder {
    pub fn new(config: Config) -> Self {
        let agent = |timeout: Duration| {
            ureq::Agent::config_builder()
                .timeout_global(Some(timeout))
                .build()
                .into()
        };
        Self {
            config,
            child: Mutex::new(None),
            health: agent(Duration::from_millis(800)),
            embed: agent(Duration::from_secs(120)),
        }
    }

    /// Spawn the server unless one is already answering.
    ///
    /// Loading the model takes a couple of minutes, so this returns immediately;
    /// callers should gate on `is_ready()` rather than assume availability.
    pub fn start(&self) -> Result<(), String> {
        if self.is_llama_server() {
            // Reusing somebody else's server means we did not spawn it, so we
            // cannot pin its GPU. Worth saying out loud: an unpinned server
            // lands on the iGPU or llvmpipe and is ~14x slower.
            log::warn!(
                "embedding: reusing an already-running llama-server on port {} \
                 -- its GPU is whatever it was started with; expected \
                 GGML_VK_VISIBLE_DEVICES={}",
                self.config.port,
                self.config.device
            );
            return Ok(());
        }
        // Something answers on the port but is not a llama.cpp server. Spawning
        // would fail to bind, and reusing it would make every embed request fail
        // with a confusing parse error, so say exactly what is wrong instead
        // (BM8). A plain `/health` 200 is not proof: any HTTP service can serve
        // `/health`.
        if self.health.get(&self.config.url("/health")).call().is_ok() {
            return Err(format!(
                "port {} is serving something that is not llama-server; set \
                 XREENSHOTY_EMBED_PORT to a free port",
                self.config.port
            ));
        }
        self.config.files_present()?;
        log::info!(
            "embedding: starting llama-server with GGML_VK_VISIBLE_DEVICES={}",
            self.config.device
        );

        let mut lock = self.child.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(child) = lock.as_mut() {
            // Already spawned; is it simply still loading?
            if matches!(child.try_wait(), Ok(None)) {
                return Ok(());
            }
            *lock = None;
        }

        let stdout = std::fs::File::create(dir_config_log()).map_err(|e| format!("log file: {e}"))?;
        let err = stdout.try_clone().map_err(|e| format!("log clone: {e}"))?;

        let child = Command::new(&self.config.server_bin)
            .arg("-m")
            .arg(&self.config.model)
            .arg("--mmproj")
            .arg(&self.config.mmproj)
            .arg("--embeddings")
            .arg("--pooling")
            .arg("mean")
            .arg("--image-max-tokens")
            .arg(self.config.image_max_tokens.to_string())
            .arg("-ngl")
            .arg("999")
            .arg("-c")
            .arg("4096")
            .arg("-b")
            .arg("4096")
            .arg("-ub")
            .arg("4096")
            .arg("--host")
            .arg(&self.config.host)
            .arg("--port")
            .arg(self.config.port.to_string())
            // Inference belongs on the discrete GPU; the iGPU stays free for
            // the desktop compositor.
            .env("GGML_VK_VISIBLE_DEVICES", &self.config.device)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(err))
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", self.config.server_bin.display()))?;

        log::info!(
            "started embedding server (pid {}), loading model on device {}",
            child.id(),
            self.config.device
        );
        *lock = Some(child);
        Ok(())
    }

    pub fn is_ready(&self) -> bool {
        self.health
            .get(&self.config.url("/health"))
            .call()
            .is_ok()
    }

    /// Label of the model currently configured, recorded against every vector it
    /// produces so a later model change is detected (BM7).
    pub fn model_label(&self) -> String {
        self.config.label()
    }

    /// True only when the port is answered by a llama.cpp server, identified by
    /// its `/v1/models` list (a non-empty `data` array). `/health` alone is not
    /// identity: an unrelated service on the port can answer it too (BM8).
    pub fn is_llama_server(&self) -> bool {
        let Ok(response) = self.health.get(&self.config.url("/v1/models")).call() else {
            return false;
        };
        let Ok(raw) = response.into_body().read_to_string() else {
            return false;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return false;
        };
        json["data"].as_array().is_some_and(|models| !models.is_empty())
    }

    /// Block until the server answers `/health`, or give up.
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if self.is_ready() {
                return true;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        false
    }

    /// Kills the server we spawned, if we spawned one. Idempotent.
    ///
    /// Called explicitly on shutdown: `child` is held behind a process-lifetime
    /// `OnceLock<Arc<Embedder>>`, so the last `Arc` never drops and `Drop`
    /// alone would never run. `child` is only ever `Some` when *we* spawned it,
    /// so a reused server is never killed.
    pub fn shutdown(&self) {
        let mut lock = self.child.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut child) = lock.take() {
            log::info!("embedding: stopping llama-server (pid {})", child.id());
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Embed a batch of inputs against `/v1/embeddings`.
    ///
    /// Images **only** work on this endpoint, and only in the wrapped OpenAI
    /// shape: `input` is an array whose entries are either plain strings (text)
    /// or `{"content": [parts]}`. A bare content array is rejected, and the
    /// older `/embedding` endpoint silently drops image parts and returns the
    /// embedding of the text field alone.
    fn post(&self, inputs: Vec<serde_json::Value>) -> Result<Vec<Vec<f32>>, String> {
        let payload = serde_json::json!({ "input": inputs, "encoding_format": "float" })
            .to_string();

        let response = self
            .embed
            .post(&self.config.url("/v1/embeddings"))
            .header("Content-Type", "application/json")
            .send(payload.as_bytes())
            .map_err(|e| format!("embedding request failed: {e}"))?;

        let raw = response
            .into_body()
            .read_to_string()
            .map_err(|e| format!("embedding body unreadable: {e}"))?;
        let json: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("embedding response not JSON: {e}"))?;

        json["data"]
            .as_array()
            .ok_or_else(|| format!("embedding response had no data: {raw}"))?
            .iter()
            .map(|entry| {
                entry["embedding"]
                    .as_array()
                    .ok_or_else(|| "embedding was not an array".to_string())?
                    .iter()
                    .map(|v| v.as_f64().map(|f| f as f32))
                    .collect::<Option<Vec<f32>>>()
                    .ok_or_else(|| "embedding contained a non-number".to_string())
            })
            .collect()
    }

    /// Embed an image file's raw bytes (PNG today, plus JPEG/WebP/GIF once
    /// `import_existing` admits them).
    ///
    /// One image per request deliberately: sending 8 in a single call measured
    /// 0.96x versus sequential, because the GPU is already saturated and the
    /// batched response cannot return until the slowest of the 8 is done.
    ///
    /// The bytes are passed through unmodified. The data URL below is *labelled*
    /// `image/png`, and llama.cpp sniffs the real format from the bytes rather
    /// than trusting that label: verified on this box, the same JPEG sent as
    /// `image/png` and as `image/jpeg` produced cos = 1.000000. So a pasted JPEG
    /// needs no re-encoding before embedding. Do not "fix" this into a
    /// per-extension MIME map without re-measuring — the label is inert.
    pub fn embed_image(&self, png: &[u8]) -> Result<Vec<f32>, String> {
        self.post(vec![image_input(png)])?
            .pop()
            .ok_or_else(|| "no vector returned".into())
    }

    /// Embed a search query. The task prefix is not optional: without it the
    /// model barely discriminates (measured 0.844 for a related pair vs 0.775
    /// for an unrelated one; with it, 0.831 vs 0.693).
    pub fn embed_query(&self, query: &str) -> Result<Vec<f32>, String> {
        self.post(vec![query_input(query)])?
            .pop()
            .ok_or_else(|| "no vector returned".into())
    }
}

/// Backstop for the normal case. Shutdown calls `shutdown()` explicitly
/// because the `Arc` lives in a `OnceLock` and so never drops, but if that
/// ref is ever released this makes sure the child cannot outlive the embedder.
/// Rust does not reap or terminate children on drop, so an ~825 MB model plus
/// its GPU allocation would otherwise stay resident after the app quits.
impl Drop for Embedder {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// One image input for `/v1/embeddings`.
///
/// The `{"content": [...]}` wrapper is load-bearing: a bare content array is
/// rejected, and the legacy `/embedding` endpoint accepts neither, quietly
/// returning the embedding of an empty text field instead.
fn image_input(png: &[u8]) -> serde_json::Value {
    serde_json::json!({
        "content": [{
            "type": "image_url",
            "image_url": { "url": format!("data:image/png;base64,{}", STANDARD.encode(png)) }
        }]
    })
}

fn query_input(query: &str) -> serde_json::Value {
    serde_json::json!(format!("{QUERY_PREFIX}{query}"))
}

/// EmbeddingGemma 2 is trained with task prefixes on text. Omitting this
/// collapses the discrimination: measured 0.844 related vs 0.775 unrelated
/// without it, against 0.831 vs 0.693 with it.
pub const QUERY_PREFIX: &str = "task: search result | query: ";

/// Little-endian f32 blob, the layout `vec0` stores.
pub fn to_f32_bytes(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for v in vector {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

/// Inverse of `to_f32_bytes`: decodes a vec0 blob back into floats.
/// Trailing partial chunks are ignored (defensive; vec0 writes aligned).
pub fn from_f32_bytes(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn dir_config_log() -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("xreenshoty")
        .join("embed-server.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BH3: the spawned server must not outlive the app.
///
/// What distinguishes a working `kill()` from a missing one is *timing*: with
/// the kill, `wait()` reaps the child immediately; without it, `wait()` blocks
/// until the child exits on its own. So the assertion is that shutdown returns
/// well before the stand-in's own lifetime, not merely that the pid is gone --
/// a plain pid check passes either way, just 20s later.
#[test]
    fn shutdown_kills_the_server_we_spawned() {
        let embedder = Embedder::new(Config::discover());
        let child = Command::new("sleep").arg("20").spawn().unwrap();
        let pid = child.id();
        *embedder.child.lock().unwrap() = Some(child);

        let started = std::time::Instant::now();
        embedder.shutdown();
        let took = started.elapsed();

        assert!(
            took < Duration::from_secs(5),
            "pid {pid} survived: shutdown waited {took:?} instead of killing it"
        );
        // Idempotent: a second call must not panic on the now-empty slot.
        embedder.shutdown();
    }

    /// A server we did not spawn is not ours to kill.
    #[test]
    fn shutdown_leaves_an_unspawned_embedder_alone() {
        let embedder = Embedder::new(Config::discover());
        // No child stored at all -- the reuse path. Must be a no-op.
        embedder.shutdown();
        assert!(embedder.child.lock().unwrap().is_none());
    }

    #[test]
    fn f32_bytes_round_trip() {
        let v = vec![0.5f32, -1.25, 0.0];
        let bytes = to_f32_bytes(&v);
        assert_eq!(bytes.len(), v.len() * 4);
        let back: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(back, v);
    }

    /// Guards the exact failure that cost us a full backfill: images must be
    /// wrapped in `content`, not sent bare or to the legacy endpoint.
    #[test]
    fn image_input_uses_wrapped_openai_shape() {
        let v = image_input(b"\x89PNG");
        let parts = &v["content"];
        assert!(parts.is_array(), "content must be an array of parts");
        assert_eq!(parts[0]["type"], "image_url");
        let url = parts[0]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "got {url}");
        assert!(url.len() > "data:image/png;base64,".len());
    }

    #[test]
    fn query_input_carries_the_task_prefix() {
        assert_eq!(query_input("cats"), format!("{QUERY_PREFIX}cats"));
    }

    /// The pick must survive a machine whose discrete GPU is not at index 1.
    #[test]
    fn resolve_prefers_a_discrete_device_over_the_iGPU() {
        assert_eq!(
            resolve_device_from(
                "  deviceName = Intel(R) UHD Graphics (ADL-S GT0.5)\n  deviceName = Intel(R) Arc(TM) A530M Graphics (DG2)\n  deviceName = llvmpipe (LLVM 19.1.7, 256 bits)\n",
            ),
            "1"
        );
        // Order flipped: discrete first, iGPU second. Must follow the name.
        assert_eq!(
            resolve_device_from(
                "  deviceName = NVIDIA GeForce RTX 4070\n  deviceName = Intel(R) UHD Graphics\n",
            ),
            "0"
        );
        // No discrete part at all.
        assert_eq!(
            resolve_device_from("  deviceName = Intel(R) UHD Graphics\n  deviceName = llvmpipe\n"),
            "0"
        );
        // Nothing parseable -> fall back to the historical default.
        assert_eq!(resolve_device_from(""), "1");
    }

    #[test]
    fn discovers_config_from_environment() {
        unsafe {
            std::env::set_var("XREENSHOTY_EMBED_DEVICE", "0");
        }
        assert_eq!(Config::discover().device, "0");
        unsafe {
            std::env::remove_var("XREENSHOTY_EMBED_DEVICE");
        }
    }
}