use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};

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
            port: 8080,
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
    /// detected instead of silently mixing incompatible embeddings.
    pub fn model_label() -> &'static str {
        "embeddinggemma-2-q8_0"
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
        if self.is_ready() {
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

    /// Embed a raw PNG. Images carry no task prefix -- the model was trained
    /// with prefixes for text only.
    ///
    /// One image per request deliberately: sending 8 in a single call measured
    /// 0.96x versus sequential, because the GPU is already saturated and the
    /// batched response cannot return until the slowest of the 8 is done.
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

fn dir_config_log() -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("xreenshoty")
        .join("embed-server.log")
}

#[cfg(test)]
mod tests {
    use super::*;

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