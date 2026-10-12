<div align="center">
<img src="./public/logo.svg" width="100" />
<h1>xreenshoty</h1>
</div>

📸 Local-first screenshots you can actually find again. Fully offline.

![offline](https://img.shields.io/badge/offline-100%25-brightgreen) ![Tauri 2](https://img.shields.io/badge/Tauri-2-blue) ![React 19](https://img.shields.io/badge/React-19-61dafb) ![SQLite + sqlite-vec](https://img.shields.io/badge/search-FTS5%20%2B%20vec0-orange) ![license](https://img.shields.io/badge/license-MIT-lightgrey)

Stop scrolling through 370 files named `Screenshot_2026-03-14.png`. xreenshoty captures your Linux desktop, reads the text in every shot, embeds what each shot looks like, and lets you search both at once. No cloud. No account. Nothing leaves the machine.

The files stay yours. PNGs live in `~/Pictures`. SQLite at `~/.local/share/xreenshoty/screenshots.db` holds metadata only. Delete the DB and the indexer rebuilds it from the PNGs.

## How it works

```text
capture --> OCR --> embed --> FTS5 + vec0 --> hybrid fuse --> answer
  |         |        |             |                |              |
X11 shot  tesseract EmbeddingGemma SQLite holds   min-max       Tauri UI
screenshots 5.5 OCR  2 768-d      text + vectors  0.44 text /
crop from   boxes +  llama-server in one row      0.56 visual
frozen      text      Vulkan GPU                   RRF optional
backdrop
```

Capture funnels through one function. Every mode calls `save_png`, which writes the row and pokes the worker. The worker OCRs with tesseract, embeds with EmbeddingGemma 2 over local HTTP, and writes FTS5 rows plus vec0 rows in one transaction.

Search runs two arms and fuses them. BM25 scans OCR text. k-NN scans image vectors with cosine distance. Each arm rescales with min-max, then blends 0.44 text and 0.56 visual. A doc missing from one arm takes that arm floor value, so text-free screenshots stay findable. Scores print as one number. The bar and the label read from the same value.

Answer synthesis is next. Top 6 hybrid hits will feed a small local model with citations and an explicit unknown state.

## Measured on this machine

Real timings from the dev box, Intel Arc A530M plus UHD 770 iGPU, 370 screenshots indexed. Your numbers will differ, but the ratios hold.

| What                        | Result                                                                                                                            |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| Corpus                      | 370 shots, 768 dims each, about 1.1 MB of vectors                                                                                 |
| Text-free shots             | 74 of 370 have empty OCR text and rely on visual search                                                                           |
| Embed speed, dGPU vs iGPU   | 1.2 s per image on Arc A530M, 17.2 s on UHD 770                                                                                   |
| Vision token cap            | Full 924 tokens takes 9.25 s. Cap 320 takes 1.43 s and keeps top 1 on 6 of 6 test shots. Cap 128 takes 0.47 s but drops to 5 of 6 |
| OCR parallelism             | 4 tesseract workers cut 12 core time from 0.95 s to 0.50 s per image                                                              |
| Task prefix for text embeds | With prefix, related pairs score 0.831 and unrelated score 0.693. Without it the gap almost closes                                |
| Fusion weights              | 0.44 OCR and 0.56 visual, set by preference. No relevance labels yet                                                              |

## Stack

| Concern        | Choice                                | Notes                                                       |
| -------------- | ------------------------------------- | ----------------------------------------------------------- |
| Desktop        | Tauri 2, React 19, Tailwind 4         | Local app, no server component                              |
| Capture        | screenshots 0.8 on X11                | xcap waits until MX repos sync libpipewire dev              |
| Files          | ~/Pictures                            | Source of truth                                             |
| Metadata       | rusqlite with bundled SQLite          | Single file, idempotent schema                              |
| Keyword search | FTS5                                  | Terms are split and quoted so operators cannot break syntax |
| OCR            | tesseract 5.5.0                       | CPU only, starts in milliseconds                            |
| Embeddings     | EmbeddingGemma 2 through llama-server | 768-d, Vulkan backend, mean pooling                         |
| Vectors        | sqlite-vec vec0 with cosine metric    | Text and vectors live in one transaction                    |
| Voice          | Whistle 16.9 MB via cactus-needle     | Push to talk, isolated venv, milestone 6 only               |

No Python in the main path. Rust talks HTTP to localhost for embeddings. Voice shells out to a venv only when you press the mic.

## Quickstart

Requirements.

- Linux with X11. XFCE tested, Wayland is not a target yet
- Bun and Rust stable
- tesseract 5 with eng data, plus vulkaninfo for the GPU pick
- Embedding model under ~/apps, llama-server binary b11541 or newer. Older builds lack EmbeddingGemma 2 support

Run it.

```bash
bun install
bun run tauri dev
```

Check it.

```bash
bun run typecheck
bun run lint
cd src-tauri && cargo check
```

Point it at models.

```bash
GGML_VK_VISIBLE_DEVICES=1 ~/apps/llama-b11541/llama-server \
  -m ~/apps/models/embeddinggemma-2-Q8_0.gguf \
  --mmproj ~/apps/models/mmproj-embeddinggemma-2-Q8_0.gguf \
  --embeddings --pooling mean -ngl 999 -c 4096 -b 4096 -ub 4096 \
  --image-max-tokens 320 --host 127.0.0.1 --port 8080
```

The app spawns this itself if port 8080 is free and kills it on exit, so the 825 MB model never sits resident. If you start the server by hand the app reuses it.

Useful env vars.

```bash
XREENSHOTY_EMBED_DEVICE=0        # override auto GPU pick
XREENSHOTY_IMAGE_MAX_TOKENS=320  # vision token cap
XREENSHOTY_FUSION_WEIGHTS="0.44,0.56"
XREENSHOTY_FUSION=rrf            # try RRF fusion instead of weighted
XREENSHOTY_TEST_DB=/tmp/test.db  # keep tests off your real index
```

## Project status

Done. Capture for fullscreen, window, and region. SQLite index with import on launch. OCR backfill. Image embeddings on dGPU. Hybrid search with fusion tests. History grid, search UI, shared thumbnails, in-app viewer, global shortcuts, voice push to talk.

Next. Answer synthesis from top 6 hits through a local 1B model, then rerank only if the eval set proves top 10 holds answers that top 3 misses.

## Contributing

Issues and PRs welcome. Run typecheck and lint before you push. Keep capture off the inference path, keep PNGs as source of truth, and add a test when you touch fusion or schema code.

## License

MIT. Built offline on Linux, for people who take a lot of screenshots.
