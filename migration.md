# Porting xreenshoty to Windows and macOS

Everything that stands between "Linux/X11 only" and a working Windows or macOS
build, per OS, with the file and line you have to touch.

The verdict first: **it is doable, and less is broken than you'd expect.** The
capture crate already in use (`screenshots = "0.8"`) is itself cross-platform —
its own README calls it "a cross-platform screenshots library for MacOS,
Windows, Linux" — so `Screen::all()`, `capture()` and `capture_area()` compile
and run on both today. `dirs::picture_dir()`, `data_local_dir()` and
`cache_dir()` are all portable, so the save folder, the DB path and the log
path need zero changes. Global shortcuts are already in Tauri's portable
accelerator format. Icons already include `icon.icns` and `icon.ico`.

What is *not* portable is everything behind a `cfg`. That is the whole work
list, and it is ordered by how badly each item bites.

**Read "Unverified" at the bottom before you start.** One Tier 1 item is
blocked on a fact nobody has checked yet.

---

## Where things stand

| Feature | Linux/X11 | Windows | macOS |
|---|---|---|---|
| Fullscreen capture | ✅ tested | 🟡 probably, see T1.0 | 🟡 needs TCC grant |
| Active-window capture | ✅ tested | ❌ stub returns `Err` | ❌ stub returns `Err` |
| Region capture | ✅ tested | 🟡 broken past scale 1 | 🟡 broken on Retina |
| Pointer → correct monitor | ✅ tested | ❌ always monitor 0 | ❌ always monitor 0 |
| Save folder / DB / logs | ✅ | ✅ portable already | ✅ portable already |
| Global shortcuts | ✅ | 🟡 works, wrong convention | 🟡 works, wrong convention |
| OCR | ✅ Indra OCR, 4 workers | 🟡 same code, needs the venv + models | 🟡 same code, needs the venv + models |
| Face indexing (People) | ✅ opt-in, ONNX on CPU | 🟡 same code, needs the venv + 2 ONNX files | 🟡 same code, needs the venv + 2 ONNX files |
| Image embeddings | ✅ Arc on Vulkan | ❌ hardcoded Linux path | ❌ hardcoded + no Metal path |
| Voice input | ✅ pw-record | ❌ no binary exists | ❌ no binary exists |
| Search / DB / indexer | ✅ | ✅ pure Rust + SQLite | ✅ pure Rust + SQLite |

---

## What already works unchanged

Do not rewrite these. They have no platform assumptions in them.

- **The frozen-backdrop region overlay.** All of `region.rs`. The whole design
  — grab once, render a static PNG, crop the selection out of the frozen image
  — exists because XFCE here has no compositor advertising `_NET_WM_CM_S0`, so
  per-pixel alpha renders opaque. Windows and macOS both *do* have compositors,
  so the frozen backdrop is unnecessary there, but it is not harmful either and
  it keeps the saved pixels provably identical to what was selected. Leave it.
  A real transparent overlay on those platforms is an optional improvement, not
  a prerequisite.
- **`open_file` / `open_folder`** are already `cfg`'d for `open` and `explorer`
  (`screenshot.rs:361-393`).
- **`save_png`** (`screenshot.rs:120`) funnels all three capture modes through
  one call, so indexing, notification and `poke()` work per-platform for free.
- **`dirs`-based path resolution**: `resolve_dir`, `db_path`,
  `embed-server.log`.
- **Everything in `db.rs` and `indexer.rs`.** Pure Rust plus bundled SQLite.

---

## Tier 1 — silent or hard failures

Each of these either errors out immediately or, worse, appears to succeed while
producing a wrong result. Fix these before claiming a port works.

### T1.0 Unresolved: which capture API does `screenshots` use on Windows?

**This is the single biggest open risk and it can invalidate the "probably
works" row above.**

`screenshots 0.8.10`'s `src/platform/windows.rs` could not be read during this
survey (the upstream path 404s and docs.rs will not render it), so the backend
is **unconfirmed**. Crates of that vintage typically use GDI `BitBlt`. If so:

- Hardware-accelerated and fullscreen content comes back **black**. Games,
  video players, GPU-composited apps.
- There is **no error and no permission prompt**. You save a black PNG and the
  indexer happily OCRs and embeds it. This is the same failure shape as the
  macOS TCC problem below, only with nothing at all to explain it.

That is the exact class of silent bug `AGENTS.md` already warns about for
embeddings ("two different images embedding identically looks like success").
The same discipline applies: **do not trust a capture that did not verify its
own mean luma.** `region.rs`'s test already asserts on mean luma rather than
"has non-white pixels" for precisely this reason — extend that idea to a
capture-time sanity check.

**Action before anything else:** build for Windows, capture a video player and
a game, and look at the pixels. If they are black, `screenshots` is out and you
need `xcap` (Windows.Graphics.Capture) or `screencapturekit-rs`. This answer
changes the whole approach, which is why it is item zero and not a footnote.

### T1.1 macOS Screen Recording permission (TCC)

**The user is never prompted correctly by default, and denial produces a black
desktop instead of an error.**

macOS requires an explicit grant in **System Settings → Privacy & Security →
Screen Recording**. Without it the app may capture *its own window* and nothing
else — the rest of the desktop is black. Independently confirmed in two places:

- The Tauri + ScreenCaptureKit example README: "The app requires **Screen
  Recording** permission on macOS", with instructions to enable it and
  **restart the app**.
- A separate writeup on cross-platform Tauri screenshots: "If the user doesn't
  authorize, the application can only capture screenshots of the application
  itself and an empty desktop screenshot."

Two properties that make this genuinely awkward:

1. **The grant is keyed to bundle identifier *and* signature.** `tauri dev` and
   a signed release build are two different apps as far as TCC is concerned and
   both need separate grants. Re-signing can reset the grant.
2. **A restart is required** after granting, so "toggled it, still black" is
   expected behaviour, not a bug.

**What to build:** preflight before capturing (CoreGraphics exposes
`CGPreflightScreenCaptureAccess` / `CGRequestScreenCaptureAccess` — symbol
names not confirmed via docs in this survey, see Unverified). If the preflight
says no, **fail loudly with a message naming the exact pane and saying a
restart is needed**, and do not write the frame. Never index a black capture.

ScreenCaptureKit also wants **macOS 14.0+** for screenshot use, which is worth
a stated minimum in the README.

### T1.2 HiDPI — the code explicitly assumes 1:1 pixels

**`AGENTS.md` already flags this as a known assumption: "Coordinate mapping
assumes 1:1 CSS pixels... Fine at scale 1; revisit if you enable HiDPI."**

Windows (125/150/175% per-monitor DPI) and macOS Retina (2x) both violate it,
and the region flow breaks in three places *simultaneously*, which makes it look
like one mystery bug:

1. **Overlay is sized in logical units, backdrop is physical pixels.**
   `region.rs:95` calls `.inner_size(w as f64, h as f64)` with the monitor's
   physical pixel dimensions. At 2x the window is half the size it needs to be,
   so the backdrop does not fill it.
2. **`clientX`/`clientY` are logical; `crop_imm` indexes physical.**
   `region.rs:148` crops at the raw coordinates the overlay sends. So the saved
   region is both offset and wrongly scaled.
3. **`capture_virtual_rect` mixes coordinate spaces** (`screenshot.rs:82-118`)
   — it treats `x`/`y` as virtual-desktop while the screen lookup and
   `capture_area` are monitor-relative.

**Fix shape:** thread a per-monitor scale factor through the region path. Either
size the window in physical pixels, or divide incoming coordinates before the
crop. On Windows additionally **confirm Tauri's per-monitor-v2 DPI awareness is
active** — if Windows decides the app is DPI-unaware it will virtualize
coordinates on your behalf and the resulting offset becomes very hard to trace.

### T1.3 Active-window capture returns an error string

**`screenshot.rs:317-320` is `Err("Active-window capture is Linux-only in this
milestone...")`.** The current implementation is x11rb reading
`_NET_ACTIVE_WINDOW` plus `_NET_FRAME_EXTENTS` and translating reparented-WM
coordinates (`screenshot.rs:255-315`). None of that has an equivalent on either
platform, and the frame-extents trick in particular does not map cleanly.

**Windows:** `GetForegroundWindow` for the handle, then
`DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)` for geometry. Note this is
*not* a drop-in for `_NET_FRAME_EXTENTS`: `DWMWA_EXTENDED_FRAME_BOUNDS`
**includes the drop shadow**, so a like-for-like port needs
`DWMWA_VISIBLE_FRAME_BORDER_RECT` or explicit shadow trimming, otherwise every
Windows capture has a transparent grey halo the Linux one does not.

**macOS:** `CGWindowListCopyWindowInfo` filtered by window layer and owning PID,
or ScreenCaptureKit's `SCContentFilter`. The ScreenCaptureKit example shows the
pattern — `SCShareableContent::get()`, then
`SCContentFilter::create().with_display(..)`, then `SCStreamConfiguration` +
`SCScreenshotManager::capture_image`. This path also needs the T1.1 grant.

**Or take the shortcut:** `xcap` exposes `Window::all()` and
`window.capture_image()` on Windows and macOS, which replaces all of this with a
library call. See "The capture-library fork" below.

### T1.4 Pointer → correct monitor is hardcoded to monitor 0

**`screenshot.rs:223-237`, the non-Linux `screens_for_pointer`, just returns
`screens.first()`.** On any multi-monitor Windows or macOS machine the region
overlay therefore always opens on monitor 0 regardless of where the cursor
actually is — and since `begin_region_select` hides the main window first, the
user gets a fullscreen drag surface on the wrong screen.

The Linux version queries the pointer via x11rb `query_pointer` and matches it
against monitor bounds (`screenshot.rs:187-221`). The equivalents:

- **Windows:** `GetCursorPos` + `MonitorFromPoint`.
- **macOS:** `NSEvent::mouseLocation` (note: Cocoa's origin is bottom-left, so
  it needs flipping relative to `CGDirectDisplayID` coordinates).

Also worth handling: a cursor straddling two monitors currently resolves to the
one containing its top-left, which is fine, but the `.or_else(|| screens.first())`
fallback will silently hide a genuine "no display found" bug. Keep the fallback
deliberate, not accidental.

---

## Tier 2 — features that are silently absent

### T2.1 Voice input is 100% Linux

`voice.rs` has four independent platform assumptions, so this does not fail
loudly — it just reports "pw-record failed to start".

1. **`record_bin()`** (`voice.rs:11-17`) probes for `pw-record`, falls back to
   `arecord`. Neither binary exists on Windows or macOS.
2. **The stop mechanism depends on POSIX signals.** `stop_gracefully`
   (`voice.rs:52-80`) shells out to the `kill` CLI to send SIGTERM so the
   recorder flushes a valid WAV header. There is **no `kill` binary on Windows,
   and SIGTERM has no meaning there** — this is the entire basis of the
   "truncated WAV gets rejected by the transcriber" fix.
3. **The venv python path is POSIX-shaped.** `voice.rs:88-89` looks for
   `<data_local_dir>/xreenshoty/voice-env/bin/python`. On Windows it is
   `Scripts\python.exe`.
4. **`cactus-needle` wheels** need to exist for the target platform, and the
   `HF_HUB_OFFLINE=1` weight cache path differs.

**Recommendation:** replace the subprocess recorder with the `cpal` crate. It is
cross-platform, and it eliminates problems 1 *and* 2 at once — you finalize the
WAV header in Rust instead of relying on a signal to make an external binary
flush. That also removes the `<4096` bytes "hold the mic longer" heuristic's
dependence on recorder behaviour. macOS additionally needs **microphone** TCC
permission, which is a separate grant from Screen Recording.

### T2.2 Embedding runtime is hardcoded to one Linux machine

`embedder.rs` will not find anything on Windows or macOS, and the failure is
log-only, so search silently degrades to keyword-only — which looks like "search
is working" rather than "search is broken".

**Path discovery** (`embedder.rs:29-51`) looks for
`~/apps/llama-b11541/llama-server` plus two `.gguf` models. Needs, per OS:

- **Windows:** `llama-server.exe`. The `.exe` suffix is not cosmetic — the
  current `is_file()` check in `files_present()` will simply fail.
- **macOS:** separate **arm64** and **x64** archives; pick by arch, not by
  guessing one.

**GPU device selection is the real trap.** `resolve_device()` runs
`vulkaninfo --summary` (`embedder.rs:80-88`) and `vulkaninfo` ships on neither
Windows nor macOS by default. So it hits the `Err(_)` branch and defaults to
device `"1"` — likely wrong, or nothing at all.

- **macOS:** use **Metal**. `-ngl 999` with a Metal build; there is no device
  index to pick, and `GGML_VK_VISIBLE_DEVICES` is meaningless there. The whole
  Vulkan-detection block should be `cfg`'d out on macOS.
- **Windows:** prefer the **Vulkan** build, but it has no `vulkaninfo` either —
  so `resolve_device_from()`'s "prefer a discrete GPU by name" heuristic gets no
  input. The CUDA 12/13 builds additionally require the **CUDA runtime DLLs to
  ship alongside**, or the binary fails to start.

**Re-verify the build-date trap.** `AGENTS.md` records that EmbeddingGemma 2
support merged on **2026-10-06** and that the prebuilt `b11429` is exactly one
day too old, failing with `unknown model architecture: 'gemma-embedding2'`.
`b11541` was a **local** build, so there is no official artifact to reuse.
Whichever official release you grab for Windows/macOS must be chosen by **build
date, not version number** — a higher version built before 2026-10-06 will fail
identically, and the error looks like a model problem rather than a version
problem.

llama.cpp's release assets do cover both platforms (confirmed): macOS Apple
Silicon arm64 and macOS Intel x64, plus Windows x64 CPU, CUDA 12, CUDA 13,
Vulkan, SYCL and HIP.

### T2.3 the OCR runtime is a Python venv plus model files

OCR is Indra OCR 1.1.2, which runs on ONNX Runtime. `ocr.rs` shells out to
`~/.local/share/xreenshoty/ocr-env/bin/python` (overridable with
`XREENSHOTY_OCR_PYTHON`) and the models live in `~/apps/models/indra-ocr`
(`XREENSHOTY_OCR_MODELS`). The code is portable as-is; the question is
packaging. `scripts/setup-ocr.sh` is the Linux answer and needs `uv`; for
macOS and Windows the same three steps apply (create a venv, `pip install
indra-ocr`, download the model files from the `datalab-to/indra-ocr` repo at
revision `v1.1.0`). The script itself is bundled via `bundle.resources`, so
only the interpreter and the models have to be installed. The face and voice
stacks have the same shape, so one installer could cover all three.

### T2.4 Bundle targets and code signing

`tauri.conf.json` currently declares `"targets": ["deb"]`, so nothing else gets
built. Add:

- **Windows:** `msi` and/or `nsis`.
- **macOS:** `dmg` and/or `app`.

macOS additionally needs **code signing + notarization**, or Gatekeeper blocks
the app for every user except you. That is a certificate and CI problem, not a
code problem, but it belongs on the checklist. macOS universal binaries
(arm64 + x64) are also a decision worth making deliberately.

---

## Tier 3 — polish

### T3.1 Shortcut convention

`src/lib/shortcuts.ts` already emits Tauri's portable accelerator format, so
registration works. But `eventToAccelerator` maps `metaKey → "Super"`
(`shortcuts.ts:45`), which is Linux naming — macOS convention is ⌘ (Command).
`prettyAccelerator` (`shortcuts.ts:58-71`) should render `⌘ ⌥ ⇧ ⌃` on macOS
rather than spelling out modifiers, and the stored defaults should probably be
Cmd-based there so a fresh install does not fight the OS. `lib.rs:65` already
registers the plugin under `#[cfg(desktop)]`, which correctly covers both.

### T3.2 Tests that assume POSIX binaries

The old `ocr.rs` timeout tests spawned `sleep` and `echo`, which do not exist
on Windows. They are gone with tesseract: the new `ocr.rs` tests are pure
functions (the word split, the batch JSON parse) plus one `#[ignore]`d
end-to-end test that needs the venv. The face suite still has the same
problem, so the `#[cfg(unix)]` gate is still worth adding there.

Two Windows-specific things the OCR swap introduces: the venv layout is
`Scripts\python.exe` rather than `bin/python` (the voice stack already has
this problem), and `convert_many` spawns workers with the `spawn` start
method, so the script must stay behind an `if __name__ == "__main__":` guard
or every worker re-runs the whole batch.

---

## The capture-library fork

`xcap 0.9.8` is the maintained successor and would collapse **T1.3, T1.4 and
half of T1.2** into library calls. It supports Linux (X11, Wayland), macOS and
Windows (>= 8.1), and exposes exactly the primitives needed here:

```rust
Monitor::all()                              // monitor list
Monitor::from_point(100, 100)               // monitor under the cursor → T1.4
monitor.capture_image()                     // fullscreen
monitor.capture_region(x, y, w, h)          // region
Window::all()                               // window list
window.capture_image()                      // active-window capture → T1.3
window.is_minimized()                       // guards the minimized case
```

Its own implementation table marks **screen capture ✅ and window capture ✅**
for X11, macOS and Windows. It also has a published Tauri integration pattern.

**The catch, straight out of `AGENTS.md`:** xcap's Linux build requires
`libpipewire-0.3-dev`, `libwayland-dev` and `libegl-dev`, and **apt refuses to
install pipewire-dev on this box** (runtime is `1.4.10-1~mx25+1`, repos only
ship `-dev 1.4.5`). Those are *compile-time* requirements even if you only ever
use its X11 path. **Migrating wholesale breaks the dev machine's build.**

So three options:

- **(A) Keep `screenshots`, hand-write the two per-OS functions.**
  No build risk. ~150 lines of `cfg`'d Win32 / CoreGraphics. Leaves the working
  X11 path completely untouched. **Recommended starting point.**
- **(B) Migrate to `xcap` wholesale.** Far less code, but **blocked here** until
  MX repos sync pipewire-dev.
- **(C) Split deps per target** — `xcap` for `windows`/`macos`, `screenshots`
  for `linux`, mirroring how `x11rb` is already Linux-only in `Cargo.toml:42`.
  Best long-term shape. Cost: you maintain two call-site dialects, and the
  `RgbaImage`-typed plumbing in `region.rs` and `save_png` has to stay
  crate-agnostic (keep the image type as `image::RgbaImage` and convert at the
  boundary).

Do (A) now, fold into (C) once pipewire-dev lands.

---

## Per-OS checklists

### Windows

- [ ] **T1.0 first** — verify `screenshots` on Windows is not GDI. Capture a
      video player and a game; black pixels mean `xcap` instead.
- [ ] Confirm per-monitor-v2 DPI awareness is active (`T1.2`).
- [ ] Implement `active_window_rect` via `GetForegroundWindow` +
      `DwmGetWindowAttribute`, and decide how to trim the drop shadow.
- [ ] Implement `screens_for_pointer` via `GetCursorPos` + `MonitorFromPoint`.
- [ ] Thread a scale factor through the region path.
- [ ] Ship `llama-server.exe`; add `.exe` to discovery and `files_present`.
- [ ] Decide Vulkan vs CUDA. If CUDA, bundle the runtime DLLs.
- [ ] Replace the voice recorder with `cpal`; fix the venv path to
      `Scripts\python.exe`.
- [ ] Ship or document the OCR runtime (venv + `indra-ocr` models).
- [ ] Add `msi`/`nsis` bundle targets.
- [ ] `#[cfg(unix)]`-gate the two POSIX-shell tests (`T3.2`).
- [ ] Default shortcuts to Ctrl-based (already correct), fix `prettyAccelerator`.

### macOS

- [ ] **T1.1** — implement a Screen Recording permission preflight, and make
      denial fail loudly naming System Settings → Privacy & Security → Screen
      Recording, plus "restart required". Never write a black frame.
- [ ] Retina handling: the entire `T1.2` list, at 2x.
- [ ] Implement `active_window_rect` via `CGWindowListCopyWindowInfo` or
      ScreenCaptureKit's `SCContentFilter`.
- [ ] Implement `screens_for_pointer` via `NSEvent::mouseLocation`, remembering
      the bottom-left origin flip.

---

## Unverified — check before trusting

Kept deliberately separate from the above. Everything in Tiers 1–3 was either
read in this codebase or confirmed from a primary source; these were not.

1. **`screenshots`' Windows backend API.** Could not read
   `src/platform/windows.rs` (upstream path 404s, docs.rs will not render it).
   Assumed GDI `BitBlt` for the purposes of T1.0 — **unconfirmed**. This is the
   highest-value unknown in this document; resolve it first.
2. **`screenshots`' macOS backend.** Similarly unread. Whether it uses
   CoreGraphics or ScreenCaptureKit determines whether it is affected by the
   macOS 14.0 floor.
3. **CoreGraphics permission symbols.** `CGPreflightScreenCaptureAccess` /
   `CGRequestScreenCaptureAccess` are the right shape for T1.1, but the Apple
   docs page 404'd during this survey, so confirm exact signatures and
   availability before implementing.
4. **Whether `tauri dev` and a signed release really need two separate TCC
   grants.** Strongly implied by TCC being keyed to bundle id + signature, but
   not directly observed. Verify empirically on a real Mac.
5. **Whether macOS also requires anything for a non-sandboxed app.** If you
   later enable the App Sandbox, `com.apple.security.device.screen-capture` and
   audio-input entitlements come into play. Not applicable to the current
   non-sandboxed config.
6. **Windows Vulkan llama.cpp without `vulkaninfo`.** Whether `-ngl 999` on a
   device-less invocation lands on the dGPU at all. Needs a real Windows GPU to
   test; the Linux "iGPU is 14x slower" trap may not even be reachable if the
   fallback silently picks nothing.

---

## Reference — external findings, with sources

- `screenshots 0.8.10` is cross-platform (macOS, Windows, Linux X11 + Wayland)
  and **carries an upstream deprecation notice: "Move to XCap"**. Its Linux
  runtime deps are `libxcb`, `libxrandr`, `dbus`.
- `xcap 0.9.8` supports Linux (X11, Wayland), macOS, Windows >= 8.1 and
  HarmonyOS. Its implementation table marks screen capture ✅ and window capture
  ✅ for X11, macOS and Windows; screen recording is WIP; **Wayland is marked
  ⛔ ("available, but not fully supported in some special scenarios")**. Linux
  compile deps include `libpipewire-0.3-dev`, `libwayland-dev`, `libegl-dev`,
  `libclang-dev`. Windows support comes via the `windows` crate 0.62; macOS via
  `objc2-*` / CoreGraphics.
- llama.cpp release assets include macOS Apple Silicon (arm64), macOS Intel
  (x64), Windows x64 CPU / arm64 CPU / CUDA 12 / CUDA 13 / Vulkan / SYCL / HIP.
- A cross-platform Tauri screenshot implementation using xcap reports
  ~16 ms per 200x200 region grab on Windows in an unoptimized `tauri dev`
  build, and throttles the magnifier loop to one capture per ~30 ms.
- macOS Screen Recording permission: granted manually in System Settings →
  Privacy & Security → Screen Recording, then the app must be restarted.
  Denial yields only the app's own window plus an empty desktop.
- ScreenCaptureKit needs macOS 14.0+ for screenshots.

- [ ] `cfg` out Vulkan device detection; use Metal with `-ngl 999`.
- [ ] Pick the llama.cpp archive by arch (arm64 vs x64) **and** by build date
      (> 2026-10-06).
- [ ] Replace the voice recorder with `cpal`; add **microphone** permission as a
      second, separate grant.
- [ ] Ship or document the OCR runtime (venv + `indra-ocr` models).
- [ ] Add `dmg`/`app` targets, plus signing and notarization.
- [ ] Decide universal binary vs single-arch.
- [ ] Shortcut defaults to ⌘; render `⌘ ⌥ ⇧ ⌃`.
- [ ] `#[cfg(unix)]`-gate the two POSIX-shell tests.
- [ ] State macOS 14.0+ if using ScreenCaptureKit.

