# xreenshoty — Project Plan

## Goal
Local-first screenshot capture and later retrieval, fully offline. Match XFCE
Screenshot behaviour for capture first; only then add local models for search.

Status: Milestones 1–4, 5.0-5.6 and 6.1-6.3 implemented. Remaining:
6.4 answer synthesis, 6.5 rerank if needed, 7.x face grouping.

## Stack

| Concern | Choice | Notes |
|---|---|---|
| Desktop | Tauri 2 + React 19 + Tailwind 4 | |
| Capture | `screenshots = "0.8"` | X11 today; `xcap` when MX repos sync |
| Files | `~/Pictures` | Source of truth, never the DB |
| Metadata | SQLite via `rusqlite` (bundled) | `~/.local/share/xreenshoty/screenshots.db` |
| Keyword search | SQLite FTS5 | Built into bundled SQLite |
| OCR | **tesseract 5.5.0** (already installed) | See M5 decisions |
| Embeddings | **EmbeddingGemma 2**, image → 768-d | Served by llama.cpp, Vulkan backend |
| Vector store | **sqlite-vec** (`vec0` virtual table) | Replaces LanceDB |
| Speech-to-text | **Whistle** 16.9 MB via `cactus-needle` | Replaces Phonon-2 |
| Answer model | **already in Ollama**: `LFM2.5-1.2B`, `gemma3:1b` — no new download needed. For 6.4, prefer serving the answer model from our own `llama-server` binary on demand (second port, shut down after) over adding an Ollama dependency: same GGUF runtime without a second resident daemon, and the embedder only holds the GPU for ~1-2 s per query so sequential use avoids contention |

All inference is local. Nothing leaves the machine.

### Python: isolated, not required

The Rust app talks plain HTTP to two localhost servers and shells out to two
venv pythons. No Python runs in the webview or on the capture path; each is a
separate process that owns its own environment:

| Component | Runtime | Python? |
|---|---|---|
| Embeddings | `llama-server` (native binary, Vulkan) over HTTP | No |
| Answer model | Ollama (Go binary) over HTTP | No |
| OCR | `tesseract` (C++) | No |
| Vector search | sqlite-vec, statically linked into our Rust binary | No |
| Voice (M6) | `cactus-needle` / Whistle | **Yes — isolated** |
| Faces (M7) | `onnxruntime` + 2 ONNX files | **Yes — isolated** |

**The isolation is the point, not "no Python".** A Python face stack buys far
more than a hand-written Rust one, and a venv is the cheapest way to keep it
from leaking into the rest of the app. Each venv lives under
`~/.local/share/xreenshoty/` alongside where the model files live in `~/apps/`.

A missing venv degrades gracefully, exactly like `voice.rs` erroring with
"cactus-needle not installed": the feature is skipped and logged, capture and
search keep working.

---

## Milestone 1–4 (done)

Milestones 1–3 are capture and capture UX; Milestone 4 is the SQLite index.
See `AGENTS.md` for the implementation notes and the build traps. Open
verification items are tracked in Milestone 5's "carry-over" list.

---

## Milestone 5 — OCR, embeddings, indexing

### Decisions taken (researched, not assumed)

**OCR: tesseract, not Xiaomi-OCR-0.** Xiaomi-OCR-0 is real (`SeerRay-Lab`, 0.8B,
Qwen3.5-based) but its published benchmarks are OmniDocBench / DocVQA / InfoVQA —
scanned, warped and photographed **documents**. Two problems on this machine:
1. It only runs behind an SGLang or vLLM **server process with a GPU**. There is
   no GPU here (Intel UHD iGPU), and it would add a Python runtime to a Rust app.
2. It is document-tuned, not UI-screenshot-tuned.

tesseract 5.5.0 is already installed (`eng` + `osd`), is CPU-only, starts in
milliseconds, and handles UI text fine. Revisit Xiaomi-OCR-0 only if search
quality on real screenshots is demonstrably weak.

**Vectors: sqlite-vec, not LanceDB.** 768 dims × 4 bytes = 3 KB per screenshot;
369 images ≈ 1.1 MB. LanceDB would mean a second storage engine, a second on-disk
format, and a cross-store join to recover `screenshot_id`. sqlite-vec keeps
vectors in the SQLite file we already open, so `ocr_text`, `embedding` and
`path` live in one row in one transaction. It is pre-v1 (`0.1.9`), which is the
main risk to watch (R2).

**Embeddings: llama.cpp, Vulkan backend, image input.** Confirmed the GGUF repo
`ggml-org/embeddinggemma-2-GGUF` ships both halves in Q8:

```
295.5 MB  embeddinggemma-2-Q8_0.gguf        text tower (270M)
529.1 MB  mmproj-embeddinggemma-2-Q8_0.gguf vision projector (170M + 300M audio)
          ── 824.6 MB resident total
```

That projector is the piece that decides whether images work at all, and it
exists. So `llama-server -hf ggml-org/embeddinggemma-2-GGUF:Q8_0 --mmproj
... --pooling ...` should serve image embeddings from `/v1/embeddings`.
825 MB fits the current 2 GB headroom, and fits trivially once the Arc card is
enabled.

Ollama is not an option for this model: `/api/embed` is text-only and
multimodal embeddings are still an open feature request (ollama #5304).

**Every path considered for EmbeddingGemma 2** (in preference order):

| Path | Image input | Cost | Verdict |
|---|---|---|---|
| llama.cpp Vulkan + mmproj | Yes, via `--mmproj` | one native binary, GPU offload | **Use this** |
| llama.cpp SYCL + mmproj | Yes | needs Intel oneAPI toolkit (~2 GB) | fallback if Vulkan misbehaves |
| OpenVINO | needs an ONNX export first | Intel-native, strong on Arc | consider only if Vulkan fails |
| llama.cpp CPU + mmproj | Yes | no GPU, ~825 MB in RAM | works today while GPU is sorted |
| transformers / PyTorch | Yes | Python + torch, multi-GB | only if llama.cpp cannot load the projector |
| ONNX Runtime | needs ONNX export | extra runtime | not yet |

**GPU: Intel Arc A530M is enabled and working.** Verified:
- `03:00.0 Intel DG2 [Arc A530M]` on PCIe, `renderD129`, driven by `xe`/`i915`.
- Vulkan exposes it as `PHYSICAL_DEVICE_TYPE_DISCRETE_GPU`, Mesa 25.0.7, API 1.4.305.
- **`VK_KHR_cooperative_matrix` is supported** (revision 2), so llama.cpp's Vulkan
  backend will use the XMX matrix engines. An earlier note in this plan claimed
  Mesa ≥26.1 was required for that — that was wrong for this card; it works on 25.0.7.
- 4 GB dedicated VRAM exposed as a prefetchable PCI BAR.
- No OpenCL ICD and no Level Zero device, so OpenVINO/oneAPI paths would need
  `intel-opencl-icd` installed first. The Vulkan path needs none of that.

Device ordering for llama.cpp is stable: **0 = UHD 770 iGPU, 1 = Arc A530M,
2 = llvmpipe**. Set `GGML_VK_VISIBLE_DEVICES=1` to pin inference to the Arc and
leave the iGPU for the desktop compositor.

**Speech-to-text: Whistle.** Phonon-2 is real but **MLX/Apple-Silicon only** and
English-only, so it cannot run on MX Linux x86_64. Whistle (`Cactus-Compute`) is
16.9 MB, 7 languages, 11 ms to first token, and ships with the same runtime as
`cactus-needle`, so `pip install cactus-needle` covers both.

**Answer model: already present.** Ollama 0.34.0 has `LFM2.5-1.2B-Instruct-GGUF`
(Q5_K_M, 843 MB), `gemma3:1b`, `qwen3.5:0.8b` and `qwen2.5:1.5b` pulled. All are
under 2B, which satisfies the original constraint with zero downloads. Ollama can
also use the GPU once the Arc card is visible.

### Risks

- **R1 (RESOLVED, see spike above): image embeddings work.** llama.cpp build
  11541+ serves both text and screenshots as 768-d vectors in a shared space,
  0.40 s per image on the Arc. Use the native `/embedding` endpoint with the
  `image` field; `/v1/embeddings` cannot take images.
- **R2: sqlite-vec is pre-v1** (`0.1.9`). Expect breaking changes; pin it.
- **R3: memory.** 825 MB of model must fit alongside a running desktop. Today
  ~2.4 GB RAM is available (7.5 GB total, 12 cores, 6 GB swap + 7.5 GB zram).
  With the Arc card enabled the model can live in its 4 GB of dedicated VRAM
  instead, which is the better arrangement.
- **R4 (resolved): Mesa version is fine.** An earlier draft warned that Arc
  needs Mesa ≥26.1 for `VK_KHR_cooperative_matrix`. Checked on the actual card:
  it is supported on Mesa 25.0.7, so no PPA or driver upgrade is needed.
- **R5: tesseract is English-only here.** Extra languages need
  `tesseract-ocr-<lang>` packages.

### Spike 5.0 findings (the facts, so nobody re-derives them)

**Runtime layout** (`~/apps`, outside the repo):

```
~/apps/llama-b11541/                     prebuilt Vulkan llama.cpp (87 MB)
~/apps/models/embeddinggemma-2-Q8_0.gguf         text tower  (296 MB)
~/apps/models/mmproj-embeddinggemma-2-Q8_0.gguf  projector  (530 MB)
```

**Version trap.** llama.cpp merged EmbeddingGemma 2 support in PR #30054 on
2026-10-06. The `b11429` prebuilt is from 2026-10-05 and fails with
`unknown model architecture: 'gemma-embedding2'`. Any build from before
2026-10-06 is useless. `b11541` works. There is no prebuilt newer than the
nightly, so check the build date before debugging anything else.

**Launch command:**

```bash
GGML_VK_VISIBLE_DEVICES=1 ~/apps/llama-b11541/llama-server \
  -m ~/apps/models/embeddinggemma-2-Q8_0.gguf \
  --mmproj ~/apps/models/mmproj-embeddinggemma-2-Q8_0.gguf \
  --embeddings --pooling mean -ngl 999 -c 2048 \
  --host 127.0.0.1 --port 8080
```

`--embeddings` is mandatory or every request returns
`501 This server does not support embeddings`.

**Use the native `/embedding` endpoint, not `/v1/embeddings`.** The OpenAI-shaped
endpoint rejects images outright (`"prompt" elements must be a string…`) — that
error message is misleading, it looks like a malformed body but the endpoint
simply has no image support. The native endpoint takes the image in a sibling
field:

```
POST http://127.0.0.1:8080/embedding
{"content": "<prompt text>", "image": "<base64 png>"}
```

Response is a list: `[{"index":0,"embedding":[[ ...768 floats... ]]}]` — note the
extra batch nesting, which trips naive parsers.

**Measured results** (Arc A530M, Vulkan1, 12 cores):

| Input | Dims | Latency |
|---|---|---|
| Text query | 768 | 0.04 s |
| Screenshot PNG | 768 | 0.40 s |

**Shared-space sanity check.** Embedding a real screenshot region and comparing
cosine similarity against three text probes:

| Probe | Similarity |
|---|---|
| "a cropped screenshot of a computer screen" | **+0.9176** |
| "a photograph of a cat outdoors" | +0.7602 |
| "a spreadsheet of financial figures" | +0.7226 |

The screenshot sits closest to the screenshot description and furthest from the
cat, so image and text embeddings really do share one space. This is what makes
"find that screenshot where…"-style search possible.

**Throughput implication.** At 0.40 s/image the first backfill of 369 existing
PNGs is roughly **2.5 minutes**. Indexing is cheap enough to do synchronously in
a background task without hurting the machine.

### Work slices

- [x] **5.0 Spike R1 — embedding runtime. ANSWER: image embeddings WORK.**
      Measured on the Arc A530M, 2026-10-10. See "Spike 5.0 findings" below.
      **Gates 5.4 → unblocked.**
- [x] **5.1 Schema.** Columns `ocr_text`, `ocr_boxes`, `embedded_at`,
      `embed_model` (added idempotently via `PRAGMA table_info`, since
      `ALTER TABLE ADD COLUMN` has no `IF NOT EXISTS` and the schema runs
      against existing databases). FTS5 table `screenshots_fts` over `ocr_text`
      using external content, kept in sync by three triggers. `vec0` table
      `screenshot_vec` at `float[768]`. sqlite-vec is registered before any
      connection opens. Both search tables are best-effort: a failure there
      disables search rather than blocking capture, because the PNGs on disk are
      the source of truth. Covered by `search_schema_is_complete_and_live`.
- [x] 5.2 Async indexer (`src-tauri/src/indexer.rs`). Background task reads
      rows where `ocr_text` is empty or `embedded_at` is NULL, runs tesseract and
      the embedding server, and writes results. Never on the capture path —
      `save_png` records the row and calls `indexer::poke()`, which only wakes
      the worker. Verified end to end on a real screenshot: OCR extracted 299
      words, embedding returned 768 dims, FTS matched the stored text and vec0
      returned the right row.
- [x] 5.3 OCR (`src-tauri/src/ocr.rs`). tesseract TSV output, which yields word
      boxes and confidences for free; only level-5 rows are real words.
- [x] 5.4 Embeddings (`src-tauri/src/embedder.rs`). The app now **spawns
      llama-server itself**, waits for `/health`, and posts to the native
      `/embedding`. If the server is unavailable the row is stamped `skipped`
      so it is retried later rather than looping forever. Model runs on the Arc
      via `GGML_VK_VISIBLE_DEVICES`.
- [x] 5.5 FTS5 keyword search UI over `ocr_text`. **Sanitise the query**:
      FTS5 treats `-`, `"`, `*`, `:` and `OR` as operators, so raw user input
      will otherwise throw a syntax error. Quote each term or use a parser.
      Done: `db::fts_query` splits on non-alphanumerics and wraps each term in
      quotes (so nothing can be reinterpreted as an operator), joined with `OR`.
      `search_text` command + Search tab, debounced 250ms. Verified against the
      real corpus: `bookmarks` -> 1 hit, `error -debug` -> 3, `a:b*c` -> 3.
- [x] 5.6 Progress surfacing so a long first backfill is visible and
      interruptible rather than a silent freeze. Done: `index-progress` event
      pushed after every batch plus a 4s poll, `index_status`/`index_pause`
      commands. A small stateful chip, NOT a bar: at 1.4s/image a bar sits
      empty or twitches, and the state is the useful signal. States are
      Loading model / Reading text / Indexing N of M / Paused; it disappears
      when caught up. Pause takes effect after the batch in flight finishes, so
      no vector is ever half-written.

Carry-over from earlier milestones (small, do them when convenient):
- [ ] Verify active-window capture, global shortcuts, Cancel, notification, and
      history thumbnails on real hardware.
- [ ] Multi-monitor and HiDPI drag-overlay checks (needs a second display).
- [ ] Region selection blackout: measured grab 324 ms + overlay page load.

---

## Milestone 6 — Search and answers

- [x] 6.1 Hybrid retrieval. Done: `search_hybrid` runs BM25 over OCR text and
      k-NN over image vectors and blends them **weighted**, 0.44 OCR / 0.56
      visual. Each arm is min-max rescaled first (best 1.0, worst 0.0) because
      BM25 runs 0..30 and cosine distance 0..1 and cannot be added raw. A doc
      missing from one arm takes that arm's floor, not zero, or the 74 text-free
      screenshots become unfindable. Degrades to keyword-only when the model is
      loading or unavailable. Three modes in the UI: Combined (default), OCR,
      Visual. Verified on the real corpus: "permission denied" returns 0 keyword
      hits but 5 hybrid; "error" and "login screen" rank differently under
      weighted fusion than under RRF.
      Weights are tunable without a rebuild:
      `XREENSHOTY_FUSION_WEIGHTS="0.56,0.44"` flips the lean, and
      `XREENSHOTY_FUSION=rrf` A/Bs against the old rank-only strategy, which
      `rrf_fuse` still implements. The 0.44/0.56 split is a stated preference,
      **not a measured optimum** — no relevance labels exist yet. Grounding:
      arXiv 2210.11934 finds normalised convex combination beats RRF, and RRF
      cannot distinguish a rank-1 that matched overwhelmingly from one that
      barely matched.

- [x] 6.2 Results UI: thumbnails, timestamps, dimensions, click to open. Done
      via `ShotCard`/`ShotPreview` (`src/components/shot.tsx`), shared by the
      history grid and search results.
- [x] 6.3 Voice input: push-to-talk, transcribe with Whistle (via
      `cactus-needle`), drop the text into the search box. Done via
      `src-tauri/src/voice.rs` (`voice_start`/`voice_stop`/`voice_cancel`) and
      `VoiceButton` in `src/components/search.tsx`. Recorder is `pw-record`
      16 kHz mono, `arecord` fallback; stop uses SIGTERM so the WAV header
      flushes. Transcriber is the `voice-env` venv python, offline
      (`HF_HUB_OFFLINE=1`), weights cached in `~/.cache`.
- [ ] 6.4 Answer synthesis: send retrieved OCR snippets plus the question to a
      small local model that cites screenshot ids. Researched candidate:
      **OCC-RAG** (`occ-ai/OCC-RAG-0.6B` / `1.7B`, Qwen3 base, mid-trained for
      faithful grounded QA). Emits `ANSWERABLE`/`UNANSWERABLE` and cites source
      ids natively -- abstaining matters here, because "I have no screenshot of
      that" is a correct answer. Beats gemma-3-4b-it on TAT-QA F1 (81.0 vs
      65.3) and ConFiQA faithfulness (81.4 vs 69.8) at 1.7B. GGUF Q4_K_M is
      1.11 GB. SERVING NOTE (not a second resident model by default): the
      embedder only holds the GPU for ~1-2 s per search query (plus backfill
      batches), so the answer model can be served from the same `llama-server`
      binary on a second port on demand and shut down after -- sequential, not
      concurrent. Ollama already has 1B models pulled and would work for a
      prototype, but it is itself a llama.cpp wrapper plus a resident daemon;
      shelling to our own binary avoids a new dependency and a second always-on
      process. Do not run both models resident side-by-side on the 4 GB Arc.
      FINDINGS (2026-10-12): three candidate LLM jobs evaluated against what
      `search_hybrid` actually returns today (up to 60 hits of
      `{path, created_at, snippet ~24 tokens, score}`; the LLM never sees
      pixels unless a vision model is bought):
      1. Answer synthesis (retrieve -> read -> reply): top 5-8 hits + question
         -> 2-4 sentences with screenshot-id cites, or UNANSWERABLE. Turns "5
         thumbnails to squint at" into a direct answer; abstention is the
         killer feature since "nothing matched" and "wrong match" look
         identical in a grid. ~2-5 s per explicit Ask click. Risk: 1B models
         hallucinate citations, which is what OCC-RAG is for.
      2. Query understanding (rewrite before retrieval): "red error dialog
         yesterday" -> `{keywords, visual description, date_filter}` split
         across the BM25 / vector arms. Fixes vague descriptions and mixed
         queries, but must be an explicit "enhance" button, never on the
         250 ms live debounce; marginal at 370 rows.
      3. Per-image captions (enrich once, search forever): a VLM describes each
         PNG once into a new indexed `caption` column, fixing the 74 text-free
         screenshots at the root. By far the most expensive: needs a 2-4 GB+
         vision model (not the 1B text models, not EmbeddingGemma), 30-90 min
         backfill on the Arc, plus migration + re-index. Deferred.
      TASKS (cheapest value first, each gated by the ~30 known-answer eval set
      from 6.5 -- if top-6 already contains the answer, (a) is sufficient):
      (a) `answer_query` command: top-6 hybrid hits -> prompt -> existing
          Ollama 1B (`gemma3:1b`) via HTTP, explicit Ask button in
          `search.tsx`, citations + UNANSWERABLE. Zero downloads; proves
          whether 1B faithfulness is enough.
      (b) Serve properly: same command via own `llama-server` binary, second
          port, on-demand start/stop; swap in OCC-RAG-1.7B GGUF only if (a)
          hallucinates.
      (c) Query rewrite button: only if eval shows vague queries failing.
      (d) Caption backfill: only if text-free shots stay unfindable after a-c.
- [ ] 6.5 Reranking only if 6.1 results feel wrong in practice. NOTE: a text
      cross-encoder (Qwen3-Reranker, bge, MiniLM) scores query-vs-OCR-text
      pairs only -- it never sees pixels, so it cannot rescue or boost the
      visual arm's hits (the 74 text-free screenshots hybrid exists for). On a
      pure-text RAG corpus this would be a different call; here it would
      systematically down-weight exactly what visual search adds. Measure the
      top-10-but-not-top-3 pattern on ~30 known-answer queries before adding one.

---

## Milestone 7 — Face detection and identity grouping

Adds a third retrieval modality. Screenshots containing faces (video calls,
meeting tiles, shared photos) become groupable by person, so "every screenshot
with Ravi in it" is one query instead of a scroll.

This is a research story, not a feature toggle: grouping is three separate models
plus one unsupervised step, each failing differently, and the last one is *not*
a neural network. All of it runs in the indexer worker, off the capture path.

### Decisions taken (researched, not assumed)

**Detection, embedding and clustering are three stages. Collapsing them is the
classic beginner mistake** — DeepFace and InsightFace make it look like one
function call, which hides where the quality actually comes from.

| Stage | Question | Output | Neural net? |
|---|---|---|---|
| Detection | "where are the faces?" | bbox + 5 keypoints | yes — YuNet, SCRFD |
| Embedding | "this crop as a vector" | 512-d float | yes — ArcFace, EdgeFace |
| Clustering | "which are the same person?" | cluster ids | **no** — geometry only |

**EmbeddingGemma cannot do this, and that is settled research — do not retry
it.** EmbeddingGemma is a *semantic* embedder (image-text contrastive): it
encodes what an image is *about*. Face recognition needs identity-discriminative
vectors, which only comes from a model trained with a margin loss over millions
of identities.

**FRoundation** (arXiv:2410.23831, Fraunhofer IGD) asked precisely this question
— "are foundation models ready for face recognition?" — over DINOv2 and CLIP, and
found pretrained foundation models **underperform architectures trained
specifically for the task**; LoRA fine-tuning only reaches parity with training
from scratch. arXiv:2411.12319 similarly finds CLIP needs explicit single-shot
fine-tuning before it is usable at all.

Concrete consequence: a **second, separate model**, stored in a **separate
`vec0` table**. Face vectors are 512-d and screenshot vectors are 768-d, so the
two cannot share a table, a KNN query, or a similarity score. Fusing the identity
arm into hybrid search is a 7.6 concern, not a 7.0 one.

**LFW is saturated. The benchmark that actually matters here is IJB-C.** Every
candidate scores 99.x% on LFW (edge faces, studio quality). Our inputs are small,
angled, low-contrast faces in dark-mode video tiles, which is exactly what
IJB-B/C stress.

| Tier | Detector | Recognizer | Params | LFW | IJB-B | IJB-C | License |
|---|---|---|---|---|---|---|---|
| **Cheap (default)** | **YuNet** 2026may | **EdgeFace-XS** (gamma=0.6) | **1.77 M** | **99.73** | **92.67** | **94.85** | MIT / BSD-3 |
| Cheap, no export | YuNet 2026may | yakhyo MobileNetV3-Large | 3.52 M | 99.53 | — | — | MIT |
| Heavy | SCRFD 10G-5GF | ArcFace R100 (`w600k_r50`) | ~66 M | 99.80 | — | — | non-comm. |

EdgeFace won the compact track of IJCB 2023's Efficient Face Recognition
competition (TBIOM 2024), at 154 MFLOPs and 1.77 M params. YuNet ships two ONNX
exports: `face_detection_yunet_2023mar.onnx` (fixed input shape) and
`face_detection_yunet_2026may.onnx` (symbolic dims). **Use 2026may with ONNX
Runtime** — the 2023mar one infers only at its fixed shape. YuNet is MIT, and
reaches 0.884 / 0.866 / 0.750 AP on WIDER val easy/medium/hard, detecting faces
from roughly 10x10 up to 300x300 px.

**EdgeFace publishes no ONNX weights.** Upstream ships `.pt` checkpoints only, so
the cheap tier carries an explicit export step (or the third-party
`yakhyo/edgeface-onnx`, BSD-3 weights). A fallback with no export step at all is
**yakhyo/face-recognition**, which publishes ONNX directly in its v0.0.1 release
(MIT; Sphere20 24.5 M params at 99.67 LFW).

**Alignment is not optional — this is the trap that reads as "grouping is
broken".** Recognition models expect a 112x112 crop warped so the eyes land at
fixed pixel positions, because that is how they were trained. Feed a raw bbox
crop and recall collapses *silently*: the same person lands near 0.55 similarity,
which looks like a threshold problem rather than a preprocessing one. That is why
the detector's 5 keypoints must be carried through — they *are* the anchor points
for the affine warp.

**ONNX Runtime, not PyTorch, at runtime.** `plan.md` already records ~2 GB RAM
available; a torch import is ~1.5 GB RSS and 5-10 s to boot, evicting the 825 MB
embedder. Export once, offline; never run torch in the app.

**Clustering thresholds are not free parameters, and eps=.35 is not
transferable.** Cosine distance is not threshold-free across models, so a
default borrowed from a demo is a guess. A threshold is only valid when read as
the *separation* between the same-person and different-person distributions,
measured on our own corpus — never in isolation. Two conditions must both hold,
both taken from `videre-core` (Apache-2.0, rusqlite — the same crates as this
project):

- A **minimum face side, ~80 px.** Sub-80 px faces embed into near-degenerate
  vectors that cluster together regardless of identity — one huge mixed blob
  that looks alarming and is mostly junk. Measured on real data: genuine person
  clusters sat at 100 px and above, the junk cluster at ~60 px. Hold tiny faces
  out as unassigned singletons rather than letting them form a blob.
- An **`eps` derived from measured separation**, plus a linkage backstop. Two
  clusters with similar centroids can still be different people — one library
  merged two real identities at 0.37 centroid similarity while their
  average-linkage distance was 0.84, far outside any sane `eps`. Failure modes
  to expect: mega-blob (eps too high), dust (too many singletons), and merge
  (centroid-only scoring).

**Auto-labels are never ground truth.** Cluster ids are internal; a user-given
name is the editable record. "Who is this?", renaming, and merging two clusters
are user actions, not model output.

**Opt-in, default off.** Face indexing is biometric-class work. It belongs behind
a settings toggle with a plain-language explanation, and indexed faces must be
deletable per cluster.

### Risks

- **R1 — small faces dominate.** Video-call tiles and sidebars mean most faces
  are well under 80 px. If that holds, this arm is valuable but *thin*: size it
  as an experiment, not a headline feature. Measure in 7.0 before writing any of
  7.1-7.3.
- **R2 — alignment quality.** Keypoint warping degrades on profile faces and
  occlusions, and no small model is trained for them. Measure the same-person
  similarity distribution separately for frontal vs profile before tuning eps.
- **R3 — bootstrap offline.** The venv is created once, from pip, ~10-30 MB of
  models. It must not silently break the offline guarantee or fail mysteriously
  on a fresh machine: fail with a clear message, exactly like voice.rs does.
- **R4 — GPU/RAM contention.** Never overlap the 825 MB embedder. The worker runs
  the face stage after the embedding stage, CPU-only by design.
- **R5 — `face-env` must not break `voice-env`.** Two venvs, two purposes, no
  shared requirements. Verify both work from the same clean install.

### Spike 7.0 (to run before any code)

Measurements, in order. If 7.0 fails, stop and report rather than building on it.

1. `onnxruntime` install size, venv size, cold boot time, RSS. Target < 200 MB.
2. Model sizes actually on disk: `face_detection_yunet_2026may.onnx` plus one
   recognizer. Target < 15 MB combined.
3. **Census over the ~370 existing screenshots: how many contain at least one
   face >= 80 px, and what is the face-size distribution?** This is the R1 gate
   and the go/no-go on the whole milestone.
4. Latency per image, CPU only, over that census.
5. Cosine separation: same-person aligned pairs vs different-person pairs, one
   histogram each. The `eps` actually chosen goes into this document with the
   numbers behind it — not before.
6. EdgeFace ONNX export verified against the `.pt` output on the same crop.

### Work slices

- [ ] **7.0 Spike** — the measurements above. Gates everything below it.
- [ ] **7.1 Schema.** Idempotent, per the existing `add_column` PRAGMA rule. A
      separate `face_vec` `vec0` table with **`distance_metric=cosine` stated
      explicitly** (the default is L2, which is not a similarity), plus a cluster
      table holding face ids and a user-given name. On any model change the face
      rows must be droppable and rebuildable from the PNGs, never migrated.
- [ ] **7.2 `face.rs`** — shells out to `face-env` python, mirroring `voice.rs`
      (venv discovery, JSON over stdout, graceful missing-venv error). Expose
      `min_face_px` as an env override so the R1 gate can be tuned without a
      rebuild.
- [ ] **7.3 Stage wiring in `indexer.rs`** — after the embedding stage, inside
      the same batch, never blocking capture. Record the model name next to every
      vector so the number has provenance.
- [ ] **7.4 Clustering** — `eps` from the 7.0 numbers, with the 80 px gate.
- [ ] **7.5 People view** — cluster list with a representative tile per cluster,
      click-through to screenshots, rename and merge controls.
- [ ] **7.6 "find me with <person>"** — KNN on `face_vec` seeded from one
      reference face. Only build it if 7.0 says there is enough signal.

---

## Rules

- Build and verify one slice at a time. Do not start 5.2 before the 5.0 spike
  reports back.
- Capture is never blocked by inference. Queue work; never `await` a model on
  the capture path.
- All inference local. No screenshot, query or audio leaves the machine.
- Biometric templates stay local too, and face indexing is opt-in until 7.0
  proves the small-face problem is solvable.
- Files are the source of truth. Losing the DB must never lose a PNG.
- Prefer deletion: no scaffolding for a later slice.

## Environment notes

- MX Linux (Debian trixie base), XFCE, **X11**, `XDG_SESSION_TYPE=x11`.
- **GPU: Intel Arc A530M present but NOT enabled.** Today only
  `00:02.0 Alder Lake-S [UHD Graphics]` is enumerable; `vulkaninfo` reports that
  iGPU plus llvmpipe. Enable it in BIOS, then re-check `vulkaninfo --summary`.
- 12 cores, 7.6 GB RAM but only ~2.0 GB available, 6 GB swap + 7.5 GB zram.
  Mesa 25.0.7 (cooperative matrix needs ≥26.1). No CUDA.
- Installed and usable: `tesseract 5.5.0` (eng, osd), `ollama 0.34.0`, `uv`,
  `python3`, `curl`, `gcc`, `vulkaninfo`/`vainfo`/`intel_gpu_top`. Not installed:
  `llama-server`, `llama.cpp`, `intel-opencl-icd`.
- `xcap` (successor to `screenshots`) needs `libpipewire-0.3-dev`, uninstallable
  here (runtime 1.4.10 vs repo dev 1.4.5). Stay on `screenshots` until MX syncs.
- `tauri = { version = "2" }` **must** stay in `[dependencies]`. Without it there is
  no `links = "Tauri"` edge, cargo forwards no `DEP_TAURI_*`, and
  `tauri_build::build()` fails with the misleading "missing `cargo:dev`
  instruction, please update tauri to latest".
- `typecheck` must be `tsc -b`; bare `tsc --noEmit` checks zero files because the
  root tsconfig is a solution file.
- `src-tauri/gen/schemas/` regenerates on each successful build, so new plugin
  permissions only become editor-valid after a build succeeds.