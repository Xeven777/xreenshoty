# AGENTS.md — working notes for xreenshoty

Durable, non-obvious facts learned while building this. Keep it short: only
things that cost real time to rediscover.

## What this is

Local-first screenshot capture for Linux desktops. Tauri 2 + React 19 + Tailwind 4.
Screenshots land in the user's Pictures folder; OCR/semantic search come later
(see `plan.md` for milestones). Everything must stay offline.

Status: Milestones 1–4 and 5.1/5.2/5.5 implemented. Keyword search (FTS5 over
OCR text) works against all 370 screenshots, and **image embeddings work** via
`/v1/embeddings` — see the trap below for the exact shape. No Python is needed
for the AI stack.

## Embedding runtime (verified — do not re-derive)

```bash
GGML_VK_VISIBLE_DEVICES=1 ~/apps/llama-b11541/llama-server \
  -m ~/apps/models/embeddinggemma-2-Q8_0.gguf \
  --mmproj ~/apps/models/mmproj-embeddinggemma-2-Q8_0.gguf \
  --embeddings --pooling mean -ngl 999 -c 4096 -b 4096 -ub 4096 \
  --image-max-tokens 320 \
  --host 127.0.0.1 --port 8080
```

Three traps, each of which looks like a different bug:

1. **Build date matters more than version.** llama.cpp merged EmbeddingGemma 2
   support (PR #30054) on 2026-10-06. Anything built before that fails with
   `unknown model architecture: 'gemma-embedding2'`. The prebuilt `b11429`
   (2026-10-05) is exactly one day too old. `b11541` works.
2. **`--embeddings` is mandatory**, else every request returns
   `501 This server does not support embeddings`.
3. **Image embeddings only work on `/v1/embeddings`, never `/embedding`.**
   `/embedding` silently drops image parts and returns the embedding of the
   text field alone — correct-looking 768-d vector, wrong content. The shape
   that works is the OpenAI one, with `content` **wrapped** in an object:

   ```json
   {"input": [{"content": [{"type": "image_url",
                            "image_url": {"url": "data:image/png;base64,..."}}]}],
    "encoding_format": "float"}
   ```

   A bare content array (`{"input": [{"type": "image_url", ...}]}`) is rejected.
   Response is `{"data":[{"embedding":[ ...768 floats... ]}]}` — *single* nested,
   unlike `/embedding`'s double nesting.

   For text, `"input"` entries are plain strings. **Task prefixes are not
   optional** — without `task: search result | query: ` the model barely
   discriminates (measured 0.844 related vs 0.775 unrelated; with it, 0.831 vs
   0.693). Images take no prefix.

Ollama cannot be used for this model at all: its `/api/embed` is text-only.

- **N image parts in ONE request = one pooled vector. Video embeddings work.**
  Verified on this box (b11541, `--pooling mean`): `input[0].content` accepts N
  `image_url` parts and returns **one** 768-d vector, not N. All frames
  contribute — cos(multi, each single frame) was 0.79/0.80/0.82/0.86, so the
  pooled vector matches no individual frame. Subsets differ too
  (cos[1,2] vs [1,2,3,4] = 0.92). That is the check that matters: N parts
  yielding a vector identical to frame 1 would mean silent frame-dropping, the
  same bug class as `/embedding`'s image-dropping.
  **Cost is linear in frames:** 1f = 1.01s, 2f = 1.99s, 4f = 4.16s, 8f = 8.79s
  (~1.1s/frame on the Arc). A 5-min video at 1 frame/10s = 30 frames ≈ 33s. So
  frame sampling is the knob that decides whether video indexing is viable; it
  is a throughput budget, not a quality dial.
  Video needs no model change — only ffmpeg frame extraction in front of the
  existing `embed_image` path. llama.cpp #30082 (server can't reach the
  encoders) was **closed as invalid**; its author was using the shallow shape
  that AGENTS.md already warns about.
- **Stills stay one-image-per-request.** Video pooling changes nothing about
  how stills embed: `embed_image` sends one part, gets one vector, as today.

- **An empty `ocr_text` is a finished OCR, not a pending one.** Keying
  "needs OCR" off `ocr_text` re-processes text-free screenshots (74 of the 370
  here) on every pass forever. It keys off `ocr_boxes`, which is always written
  (`[]` when nothing was found) and so records "attempted".
- **Verify embeddings are actually distinct.** Two different images embedding
  identically looks like success: dimensions are right, counts are right, the
  log says `embedded`. Only `SELECT count(*)` from `screenshot_vec` proves
  nothing. Compare cosine between two rows. Real spread over 8 screenshots is
  min 0.57 / max 0.997 — the high pair was two shots 13s apart, which is
  correct for near-duplicates. This check is what caught the silent-drop bug.
- **vec0 needs `distance_metric=cosine` stated explicitly.** The default is L2,
  which is not a similarity. The table is declared with the metric so
  `distance` is directly `1 - cosine`; `db::l2_to_cosine` exists only to
  interpret older L2 tables. Migrating the table is NOT a rename:
  `ALTER TABLE ... RENAME TO` renames the virtual table but leaves its shadow
  tables under the old prefix, and the table becomes unreadable ("Error
  preparing rowid scan: no such table: screenshot_vec_rowids"). Worse, `DROP`
  on the broken table fails and the drops roll back with the transaction, so a
  partial repair loops forever. When a vec0 table is truly broken, delete the
  database and let the indexer rebuild from the PNGs -- files are the source of
  truth.
- **Similarity must be shown as one number, not two.** The bar and its label
  must come from the same value. An earlier version scaled the bar against the
  best hit while printing the raw cosine, so "0.67" sat next to an almost-full
  bar; that reads as broken. Also never print `1/(1+distance)`: it is not a
  similarity and collapses every result into 51-59%.
- **vec0 KNN needs an aliased `k`.** `WHERE embedding MATCH ?1 AND k = 5` in a
  JOIN silently returns arbitrary rows with distance 0.0000. Write
  `v.embedding MATCH ?1 AND v.k = ?2`. `search_similar` does this.

- **GPU choice is automatic, and the iGPU is ~14x slower.** `resolve_device()`
  runs `vulkaninfo --summary` and picks the first device naming itself discrete
  (arc/nvidia/geforce/amd/radeon/apple); Vulkan index order is machine-specific,
  so a hardcoded 1 would be wrong elsewhere. Measured on identical images:
  Arc A530M 1.2 s/image vs UHD 770 17.2 s/image. Override with
  `XREENSHOTY_EMBED_DEVICE`. If the app *reuses* a server it did not spawn, it
  cannot pin the GPU and logs a warning -- start llama-server without
  `GGML_VK_VISIBLE_DEVICES` and it silently lands on the iGPU.
- **`DRI_PRIME=1` renumbers the Vulkan devices.** The XFCE session exports it,
  and Mesa then promotes the dGPU to index 0: the app sees Arc=0 / UHD=1 while
  a plain shell sees UHD=0 / Arc=1. So the app picking "device 0 (arc)" is
  correct, and it is genuinely the dGPU -- confirmed 2.5 s/image on a
  1920x1080 capture. Never hardcode an index, and never trust an index read
  from a different environment. To reproduce the app's view, run vulkaninfo with
  the app's own environ.
- **Cap vision tokens at 320.** Without `--image-max-tokens`, a full 1920x1080
  capture expands to 924 tokens and 9.3s on the Arc. Measured on the same
  image: 924 tok = 9.25s, 320 tok = 1.43s (6.5x), 128 tok = 0.47s (19.7x).
  Quality is the deciding factor, not speed: comparing nearest-neighbour
  rankings across 6 screenshots, cap-320 reproduced the full-res top-1 for
  **6/6**, cap-128 only 5/6. Override with `XREENSHOTY_IMAGE_MAX_TOKENS`.
  Earlier "27s/image" readings were contaminated by two llama-servers sharing
  the GPU -- always confirm only one is running before trusting a timing.
- **Parallelism: OCR yes, batched embeddings no.** tesseract is a subprocess,
  so 4 at a time is a real 1.9x (0.95 -> 0.50 s/image on 12 cores). Packing 8
  images into one `/v1/embeddings` request measured **0.96x** -- the GPU is
  already saturated, so a batch just blocks until the slowest finishes. Keep
  one image per request and per-image timings for free.
- **The app owns the llama-server process.** `embedder::Config::discover()` finds
  the binary/model under `~/apps`, overridable via `XREENSHOTY_LLAMA_SERVER`,
  `XREENSHOTY_EMBED_MODEL`, `XREENSHOTY_EMBED_MMPROJ`, `XREENSHOTY_EMBED_DEVICE`.
  If something already answers `/health` it is reused, not duplicated. The model
  takes ~2 min to load, so startup never blocks; a `Drop` impl kills the child so
  an 825 MB model is never left resident.
- **`vec0` does not support UPSERT.** `ON CONFLICT DO UPDATE` on a `vec0` virtual
  table fails with "UPSERT not implemented". Use DELETE + INSERT inside one
  transaction instead.
- **Hybrid search normalises before blending, and both arms degrade
  independently.** BM25 (0..30) and cosine distance (0..1) cannot be added
  raw, so each arm is min-max rescaled (lower raw = better on both) and blended
  0.44 OCR / 0.56 visual. A doc missing from one arm takes that arm's floor, not
  zero, or the 74 text-free screenshots become unfindable. **min-of-empty is
  infinity** -- an arm with no results must contribute 0.0, else every score ties
  and the id tiebreak returns an arbitrary order. RRF survives only behind
  `XREENSHOTY_FUSION=rrf` for A/B; weights move with
  `XREENSHOTY_FUSION_WEIGHTS="0.44,0.56"`. The 0.44/0.56 split is a user
  preference, not a measured optimum -- there are no relevance labels yet.
- **FTS5 `MATCH` input must be sanitised.** `-`, `"`, `*`, `:` and `OR` are query
  operators; raw user input will raise a syntax error. `db::fts_query` splits on
  non-alphanumerics and wraps each term in quotes, so no operator can survive.
- **The app log must be filtered.** Without `Builder::filter(..)` in `lib.rs`,
  `ureq` logs every HTTP byte at TRACE and the log file becomes 100% noise,
  burying the `indexer:` lines that actually diagnose problems.
- **An empty `ocr_text` is a finished OCR, not a pending one.** Keying
  "needs OCR" off `ocr_text` re-processes text-free screenshots (black regions,
  dark desktops) on every pass forever. It keys off `ocr_boxes`, which is always
  written (`[]` when nothing was found) and so records "attempted".
- **Two gates decide whether a file gets indexed, and both must be widened.**
  `is_indexable_image` (extension) and `image_dimensions` (header) are separate
  checks in `import_existing`. `image_dimensions` originally only parsed a PNG
  IHDR, so widening the extension list alone changed nothing — every JPEG still
  fell through. It now hand-parses PNG and falls back to
  `image::guess_format` for JPEG/WebP/GIF.
- **llama.cpp ignores the data-URL MIME label.** `embed_image` labels every
  payload `image/png`, and that is inert: the same JPEG sent as `image/png` and
  as `image/jpeg` returns cos = 1.000000, because the format is sniffed from the
  bytes. So a pasted JPEG is embedded correctly with no re-encoding. Do not add
  a per-extension MIME map without re-measuring.
- **AVIF is deliberately not indexed.** `image` 0.24 gates AVIF behind
  `avif-decoder`, which needs the `dav1d` crate; that builds dav1d from source
  and requires meson + nasm, neither of which is installed here. The system
  `libdav1d` 1.5.1 *is* installed (ffmpeg uses it), so AVIF is a link-against-
  system-dav1d build script away — but indexing a format `thumbnail` cannot
  decode would create a permanently broken row, so it stays out until the decode
  path exists too. Same reasoning excludes tiff and bmp.
- **The save folder is watched, not polled.** `watch.rs` holds a non-recursive
  inotify watch on the Pictures folder and runs `import_existing` + `poke()`
  once the folder goes quiet (`SETTLE` = 400 ms), coalescing the several writes a
  clipboard save performs into one directory scan. It cannot see a *custom* save
  folder: settings live in the webview's `localStorage` and Rust holds no state,
  so a custom folder still relies on `importExisting(dir)` re-running on change.
- **The indexer must chain on work done, not row count.** A full batch whose
  rows could not be processed (model still loading) returns identically on the
  next query, so re-notifying on `rows.len()` spins the worker in a tight loop
  against the DB. Guarded by `should_chain`, unit-tested.
- **Shot tiles and the viewer are shared.** `ShotCard`/`ShotPreview` in
  `src/components/shot.tsx` back both the history grid and search results;
  `usePreview()` holds the open item. Clicking a tile opens the in-app
  full-size viewer; the `open_file` command hands the original PNG to xdg-open.
- **Toasts go through `src/lib/toast.ts` (sonner).** `<Toaster>` is mounted in
  `main.tsx` *outside* `[data-ui-scroll-container]` so the scroll shell never
  clips it. Copy lives in the `toastSaved`/`toastDeleted`/`toastFailed`
  helpers — keep new feedback there, not raw `toast.*` calls. OS notifications
  (`notify.ts`) are only for background saves; in-app feedback is sonner.
- **Thumbnail/preview caches are module-level in `use-thumbnails.ts`.**
  `useThumbnails` fills a 120-entry LRU keyed by path; the full-size preview
  keeps the last 3. Both are shared across history and search — a path loads
  once per session. `ShotCard` is `memo`ized, so only cards whose thumb
  actually arrived re-render.
- **Debug events are DEV-only, and heavy payloads are truncated.** `trackedInvoke`
  skips `emitDebugEvent` outside `import.meta.env.DEV`, and `thumbnail`/
  `backdrop_data_url` results are logged as `<NKB data-url>` — JSON-cloning a
  base64 PNG per image stalls the grid (it also ran in prod builds).
- **Region overlay drag geometry is direct-DOM.** `scheduleRect` rAF-coalesces
  writes to the selection div's style; React state only flips on pointerdown/up.
  Reverting to per-move `setState` re-renders the fullscreen backdrop at drag
  rate.
- **The thumbnail loader is shared.** `useThumbnails` in `src/lib/use-thumbnails.ts`
  backs both the history grid and search results; do not re-inline the worker pool.
- **Clearing the thumbnail cache needs both caches.** Settings → Clear wipes the
  on-disk PNGs under `~/.cache/xreenshoty/thumbs` (`clear_thumbnail_cache`) *and*
  the two module-level `Map`s in `use-thumbnails.ts` (`clearMemoryCaches`). Disk
  only looks like a no-op: the maps still hold every URL the session loaded, so
  nothing re-requests. Mounted grids keep their own state and only refill on the
  next path change, so on-screen tiles do not visibly rebuild — that is
  expected, not a failure.
- **`clear_cache_in`/`cache_size_in` take the dir as a parameter** precisely so
  the test does not call the commands. Those target the real `~/.cache`; a test
  invoking them would delete the user's actual thumbnails.
- **Indexing never blocks capture.** `save_png` writes the row and calls
  `indexer::poke()`, which only wakes the worker. `indexer::APP` is a module-level
  `OnceLock<AppHandle>` so capture commands do not need one just to forward it.

## Commands

```bash
bun run typecheck      # tsc -b — MUST be -b, see trap 4
bun run lint           # eslint
bun run tauri dev      # run the desktop app (capture needs this, not the browser)
cd src-tauri && cargo check
```

Always run typecheck + lint before calling frontend work done.

## Environment (this machine)

- MX Linux, Debian trixie base, XFCE, **X11** (`XDG_SESSION_TYPE=x11`).
- **GPU: Intel Arc A530M (DG2), enabled and working.** Vulkan device order is
  0 = UHD 770 iGPU, 1 = Arc A530M, 2 = llvmpipe. Pin inference with
  `GGML_VK_VISIBLE_DEVICES=1` so the iGPU stays free for the compositor.
  Mesa 25.0.7, Vulkan 1.4.305, and `VK_KHR_cooperative_matrix` **is** supported —
  contrary to the common claim that Arc needs Mesa ≥26.1. No OpenCL ICD and no
  Level Zero device, so OpenVINO/oneAPI would need installing first.
- Wayland is not a target right now. Active-window capture is Linux/X11-only.

## Traps — read before touching the build

### 1. `tauri` MUST stay in `[dependencies]`

If `tauri = { version = "2" }` is missing or moved out of `[dependencies]` in
`src-tauri/Cargo.toml`, the build fails with:

```
missing `cargo:dev` instruction, please update tauri to latest
```

This message is a lie — tauri *is* latest. The real cause: tauri declares
`links = "Tauri"`. Cargo forwards `DEP_TAURI_*` build-script metadata only along
that edge, so with no direct dependency there is no edge, no `DEP_TAURI_DEV`,
and `tauri_build::build()` panics. Do **not** work around it by patching
`src-tauri/build.rs`; restore the dependency instead.

### 2. `xcap` is unusable on this box — stay on `screenshots`

`xcap` is the maintained successor to `screenshots`, but its Wayland path needs
`libpipewire-0.3-dev`, which apt refuses to install: the installed runtime is
`1.4.10-1~mx25+1` while MX/Debian repos only ship `-dev 1.4.5`. Revisit when
the repos sync, then migrate `Screen::all()` → `Monitor::all()` and swap
`capture()`/`capture_area()` for `capture_image()`/`capture_region()`.

### 3. `gen/schemas` is generated

`src-tauri/gen/schemas/desktop-schema.json` is rewritten on every successful
build. Editors flag new plugin permissions as invalid until a build succeeds.
That is stale schema, not a bad permission string.

### 4. `typecheck` must use `tsc -b`

Root `tsconfig.json` is a solution file (`"files": []` + project references).
Plain `tsc --noEmit` therefore checks **zero files** and always passes. Use
`tsc -b`, which is what `bun run build` runs first. The `typecheck` script is
already `tsc -b` — do not "simplify" it back to `--noEmit`.

## Implementation notes

- **Thumbnails must come from Rust.** `convertFileSrc` needs the asset protocol
  enabled in `tauri.conf.json` plus scope entries; without them previews render
  blank. The `thumbnail` command returns a base64 PNG data URL instead, so any
  save folder works with zero extra permissions.
- **Active window** uses `x11rb` directly (`_NET_ACTIVE_WINDOW` +
  `_NET_FRAME_EXTENTS`, then `translate_coordinates` for reparented WMs). The
  `active-win-pos-rs` crate is Windows/macOS only despite its name.
- **Global shortcuts reuse the click path.** `Capture` publishes its handlers
  through `actionsRef` and binds them itself, so shortcuts get delay, cancel,
  history and notifications for free. A bare key is rejected — a modifier is
  required so normal typing is never hijacked.
- **Settings persist in `localStorage`** under `xreenshoty.settings`; session
  history under `xreenshoty.history`. Rust holds no state.
- **Voice is push-to-talk through `voice.rs`.** `voice_start` spawns
  `pw-record` at 16 kHz mono (`arecord -f cd` fallback), `voice_stop` ends it
  with SIGTERM so the recorder flushes a valid WAV header -- SIGKILL leaves a
  truncated file (`fmt chunk missing`) the transcriber rejects. Transcription
  shells out to the `voice-env` venv python (`cactus-needle`;
  `needle.transcribe` returns a dict with `"text"`), with `HF_HUB_OFFLINE=1`
  so it stays offline against the `~/.cache` weights. Temp WAVs live in
  `temp_dir()` and are deleted after transcribe/cancel.
- **Files are the source of truth, not the DB.** Milestone 4 must keep PNGs in
  Pictures; SQLite is metadata only.
- **Metadata is recorded in `save_png`, not per capture mode.** All three modes
  funnel through `screenshot::save_png`, so that is the single place to index a
  capture. `db::record` logs and swallows failures on purpose: a database
  problem must never lose a PNG that is already on disk.
- **DB lives at `~/.local/share/xreenshoty/screenshots.db`.** `XREENSHOTY_TEST_DB`
  overrides the path so tests do not touch the real index. Import dedupes on the
  UNIQUE `path` column and is safe to run on every launch.
- **Frontend never touches SQL.** Only `db::list_recent`, `db::import_existing`
  and `db::screenshot_count` are exposed. Do not add `tauri-plugin-sql`.
- **Schema changes must stay idempotent.** The schema is applied on every open
  against databases that already exist. `ALTER TABLE ADD COLUMN` has no
  `IF NOT EXISTS`, so `add_column` checks `PRAGMA table_info` first. FTS5 and
  `vec0` are created best-effort and only log on failure: search degrading is
  fine, capture breaking is not.
- **sqlite-vec must be registered before `Connection::open`**, via
  `sqlite3_auto_extension`, which is process-global so once is enough. It has to
  go in as `Some(std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ()))`:
  the C function takes a zero-arg entry point (`void(*)(void)`), but rusqlite's
  Rust wrapper declares it as `fn(Option<unsafe extern "C" fn()>) -> c_int`, so
  the two signatures do not unify and `transmute` is the only bridge.
- **`XREENSHOTY_TEST_DB` is process-global.** Two database tests running in
  parallel clobber each other's path and read each other's rows; the shared
  `db::test_lock()` exists for exactly this reason. Deleting a SQLite file also
  requires deleting its `-wal` and `-shm` sidecars, or the rows replay on reopen.
- **Tests must clear a DB's WAL sidecars** — see above. A stale `-wal` makes a
  fresh test file look like it already has rows.
- **Region drag overlay is a second window, and it is NOT transparent.**
  `region.html` + `src/region.tsx` are a separate Vite entry (see
  `build.rollupOptions.input`). Flow: `begin_region_select` hides the main
  window, grabs the whole monitor under the cursor into Tauri state, shows the
  window again, then opens an opaque always-on-top overlay that renders that
  frozen PNG and emits `xreenshoty:region-selected` / `-cancelled`. The main
  window crops the rect out of the stored image via `save_region_from_backdrop`
  and closes the overlay. Two consequences to preserve: no live screen grab
  happens at save time (so the saved pixels are exactly what was selected), and
  coordinate translation is unnecessary because the crop is monitor-local.
- **Never make an overlay window transparent on this box.** XFCE has
  `use_compositing=true` but no compositor actually advertising
  `_NET_WM_CM_S0`, so `transparent(true)` renders opaque (white, since
  `index.css` sets `body { bg-background }`). The frozen-backdrop trick avoids
  the whole problem.
- **Coordinate mapping assumes 1:1 CSS pixels.** The overlay window is sized to
  the monitor and the backdrop is the full monitor, so `clientX/clientY` index
  the image directly. Fine at scale 1; revisit if you enable HiDPI.

## Conventions

- Formatting/lint is enforced by `eslint-plugin-tailwind-canonical-classes`, so
  write canonical Tailwind classes (`top-1/2` not `top-[50%]`, `bg-card` not
  `bg-[var(--card)]`).
- `react-hooks/refs` is strict: no reading or writing refs during render.
  Derive constants with `useMemo`, not `useRef(x).current`, and write refs inside
  effects.
- `invoke` goes through `trackedInvoke` in `src/lib/tauri.ts` so the dev-only
  DebugPanel records it. Use it for every new command.
- No comments unless asked.