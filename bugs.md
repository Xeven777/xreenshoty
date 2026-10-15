# xreenshoty — bug & optimization review

Findings only. Baseline: `bun run typecheck` and `bun run lint` are both clean,
and `bun run build` succeeds.

Fixed since the original pass: **H1, H2, H3, M1, M3, M5, M7, BH1, BH2, BH3, BM3,
BM4, BM6, BM9, BM10, L8, L9, L10** (18 of 43). Each is marked **Fixed** below.
Everything else is still open.

Contents:

1. **Frontend** — `src/`, `index.html`, `region.html`, `vite.config.ts`,
   `package.json` (29 findings).
2. **Backend** — `src-tauri/src/` (22 findings). The backend was read on its
   own pass; where it is cited in the frontend section it was only to confirm a
   claim, and vice versa.

Open: 1 high, 10 medium, 22 low (33 of 43).

---

## High

### H1. Region capture can wedge the app with all capture controls disabled — **Fixed**

`src/components/capture.tsx:198-216`, `:462-484`

`startRegionSelect()` sets `busy = "region"` and then hands off to the overlay.
`busy` is only cleared in `resetCapture()`, which is reached from exactly four
places: the countdown abort, the `beginRegionSelect()` catch block,
`finishCapture()`'s `finally`, and `applyRegionResult()`'s `finally`. The last
two only run if the overlay actually emits `REGION_SELECTED` / `REGION_CANCELLED`.

If the overlay opens but never emits — webview fails to boot, `backdrop_data_url`
hangs, event lost — nothing ever calls `resetCapture()`. Consequences:

- All three capture buttons stay `disabled` (`capturing === true`).
- The delay input is disabled too (`capture.tsx:505`).
- The Cancel banner is _not_ shown, because it requires `countdown !== null`
  and the countdown has already finished (`capture.tsx:462`).
- The global shortcuts no-op, because `runCapture`/`startRegionSelect` bail on
  `if (busy) return` (`capture.tsx:199`, `:263`).

The user's only recovery is restarting the app.

**Fix:** took the "visible control" option, not the watchdog — a timer that
fires while the user is mid-drag would replace a recoverable bug with a worse
one.

- The Cancel banner was gated on `countdown !== null`, which is exactly the
  state that has just ended by the time the overlay owns the screen. It now
  renders for `busy === "region"` as well, so the escape hatch is on screen
  during the selection.
- New `cancelCapture()`: during the countdown it sets `abortRef` (unchanged
  behaviour, the countdown loop polls it); once the overlay owns the screen it
  calls `closeRegionOverlay()` + `restoreMainWindow()` + `resetCapture()` — the
  exact teardown `applyRegionResult` does, so both exits are now identical.

M3 was fixed alongside this, since a lost `pointerup` was one of the ways to
reach this state in the first place.

### H2. Switching search mode shows the other mode's results as if they were current — **Fixed**

`src/components/search.tsx:40-41`, `:46-77`

```ts
const searching = tauri && term.length > 0 && term !== settled
const searched = tauri && term.length > 0 && term === settled
```

`settled` stores only the term. The effect re-runs on `[term, mode, tauri]`, but
`settled` still holds the term from the previous mode.

Trace: search `"cat"` in **Words** → `settled === "cat"`, `results` = FTS hits.
Click **What's in the picture**. The effect re-fires, but
`term === settled`, so `searching === false` and `searched === true`. For the
whole debounce + embedding round trip:

- no spinner (`search.tsx:126`),
- `shown` keeps the old text results (`:42`),
- the badge still reads "N screenshots matching" (`:146-153`).

Results then swap silently when the visual search lands.

**Fix:** `settled` now stores `${mode}:${term}` (the `key`), so a mode change
always puts the UI back into `searching`. This also closed H3 and M7 — see the
note under H3.

### H3. On search error, stale results from the previous term are shown as matches — **Fixed**

`src/components/search.tsx:61-70`

The `catch` sets the error but never clears `results`, and the `finally` still
runs `setSettled(term)`. So `searched` becomes `true` while `results` still
holds the _previous_ term's hits → `shown` renders them, and the count badge
(`:146-153`) claims they match the current query. A user seeing "12 screenshots
matching" next to an error message will read it as a successful search.

**Fix (H2 + H3 + M7 together):** `settled` is keyed `${mode}:${term}`, and the
two error strings collapsed into one `failure: { key, msg } | null` read through
`failed = searched && failure?.key === key`. Three consequences:

- `shown` and the count badge are both gated on `!failed`, so a failed query can
  never render the previous term's hits as matches.
- A mode or term change moves `key`, which retires the error by derivation — no
  `setState` in an effect, which `react-hooks/set-state-in-effect` rejects.
- The empty state reads "Search failed" instead of "Nothing matched".

Verified: `bun run lint` + `bun run build` clean.

---

## Medium

### M1. Changing the save folder never re-imports or refreshes history — **Fixed**

`src/components/capture.tsx:129-141`

```ts
const bootstrapped = useRef(false)
useEffect(() => {
  if (!tauri || !saveDir || bootstrapped.current) return
  bootstrapped.current = true
  ...
}, [tauri, saveDir, refresh])
```

`bootstrapped` latches on the _first_ non-empty `saveDir`. Editing the folder in
Settings (`settings-panel.tsx:140-146` or `capture.tsx:529-535`) does nothing to
history: `importExisting(newDir)` never runs and `refresh()` never runs. The user
captures into the new folder and the grid does not update until some later
capture happens to call `refresh()`.

**Fix:** `bootstrapped` became `importedDir`, a ref holding the last directory
_handled_ rather than a "has this ever run" boolean. Editing the save folder now
re-runs `importExisting` + `refresh`. Import is already idempotent (dedupes on
the UNIQUE `path`), so re-running on every change is free.

### M2. Changing a shortcut can leave the _old_ accelerator registered forever

`src/components/capture.tsx:292-321`

```ts
const fullError = await bindShortcut(full, ...)
if (disposed) return          // <- runs AFTER register() succeeded
```

`bindShortcut` is async (`shortcuts.ts:81-92`). Sequence when the user edits the
shortcut while the previous bind is still in flight:

1. Effect A starts, awaits `isRegistered`.
2. Setting changes → cleanup A runs: `disposed = true`, `releaseShortcut(old)`.
   But A has not registered yet, so `isRegistered(old)` is false → no-op.
3. Effect B binds the new accelerator.
4. A resumes, `register(old)` succeeds, then `if (disposed) return` skips the
   error path — and skips any release.

Result: the abandoned accelerator stays registered for the process lifetime and
still fires `actionsRef.current.fullscreen?.()`. Rapid edits stack several.

**Fix direction:** release inside the `disposed` branch
(`if (disposed) { await releaseShortcut(full); return }`), or hold a token so a
stale bind undoes itself.

### M3. Region overlay loses the drag if the pointer is released outside the window — **Fixed**

`src/components/region-overlay.tsx:51-73`, `:90-94`

`handlePointerUp` is a plain React handler on the container. There is no
`setPointerCapture()` on pointerdown and no `pointercancel` handler. Releasing
the button past the edge of the overlay's monitor (multi-monitor, or just
dragging off-screen) means `pointerup` never arrives → no `REGION_SELECTED` is
emitted → the drag is silently discarded while `anchor` stays set, so the hint
text keeps saying "Release to capture" (`:166-168`). Combined with H1, this is
also a route into the wedged state.

**Fix:** `setPointerCapture(event.pointerId)` in `handlePointerDown`, so the
release is delivered to the container even when the button is let go past the
monitor edge. Added an `onPointerCancel` handler that calls `finish(null)` for
the case capture cannot cover — the browser taking the gesture over (touch pan,
compositor grab). Both routes now always emit, which is what H1 needed to not be
reachable.

### M4. Pointer tracking re-renders the whole overlay on every mouse move

`src/components/region-overlay.tsx:91-93`

`onPointerMove` does `setCursor(...)` unconditionally — one React state update
per `pointermove`, i.e. 60–120 renders/sec while dragging. Each render
reconciles the full-monitor backdrop `<img>` (`:100-113`) and the selection box.

**Fix direction:** only store a new cursor position when it actually changed by
≥1px, or buffer moves in a ref and flush via `requestAnimationFrame`. The
selection geometry itself is cheap; the point is avoiding a render per event.

### M5. Card "open folder" opens the configured save folder, not the screenshot's folder — **Fixed**

`src/components/shot.tsx:202-211`, `capture.tsx:591-595`, `search.tsx:171`

`onOpenFolder` always calls `openFolder(dirArg)` with the _current_ save
setting. Rust-side, `open_folder` resolves and opens exactly that directory
(`src-tauri/src/screenshot.rs:342-359`). For a screenshot captured before the
save folder was changed — or one imported from elsewhere — the button opens the
wrong directory.

**Fix:** new `dirOf(path)` helper in `shot.tsx`, next to the existing `fileName`.
Both callers — the history grid and the search results, each with its card _and_
its `ShotPreview` — now open `dirOf(item.path)`. This also deleted `Search`'s
`saveDir` prop, which had no other use.

### M6. `ShotPreview` is `aria-modal` but neither traps nor moves focus

`src/components/shot.tsx:66-144`

- `role="dialog" aria-modal="true"` with no focus move into the dialog, so
  assistive tech keeps reading the grid behind it.
- No focus trap: Tab walks through the hidden background content.
- No focus restore to the tile that opened it on close.
- Background scroll is not locked.

Escape handling (`:57-64`) is the only keyboard affordance.

**Fix direction:** on open, focus the close button (or the dialog itself);
trap Tab within; restore focus to the originating element on close; add
`overflow: hidden` to the scroll container while open.

### M7. Stale error banners survive mode switches and cleared queries — **Fixed**

`src/components/search.tsx:56`, `:61-67`, `:134-144`

- `setVisualError(null)` runs only inside the visual branch; `setError(null)`
  only inside the text branch. A failed visual search keeps showing
  _"Visual search is unavailable: …"_ after the user switches back to **Words**
  and gets successful text results.
- Clearing the query returns early from the effect (`:47`) without clearing
  either error, so the banner remains visible over the empty state.

**Fix:** closed by the single `failure` state described under H3. The error is
stored against the key it belongs to, so a mode switch or a cleared query no
longer leaves a banner behind. The two branches (plain vs. "Visual search is
unavailable: ") collapsed to one banner keyed on the current mode.

### M8. Settings loaded from localStorage are not validated or clamped

`src/lib/settings.ts:29-37`, `:47-56`

`clampDelay` is applied only inside `update()` when `key === "delaySecs"`. The
load path is `{ ...DEFAULT_SETTINGS, ...JSON.parse(raw) }` with no type check.
A stale or hand-edited value such as `{"delaySecs": 9999}` or `{"delaySecs":"5"}`
is accepted verbatim:

- the number input renders `9999` and the Increase button is disabled
  (`capture.tsx:516`), but the countdown would run 9999 seconds,
- a string delay makes `delaySecs + delta` string-concatenate in `stepDelay`
  (`capture.tsx:327`).
- `notifyOnSave` is likewise not coerced to boolean, so `"false"` is truthy.

**Fix direction:** run the loaded object through a normaliser (clamp
`delaySecs`, coerce booleans, drop unknown types) before it becomes state.

### M9. `role="status"` on an interactive button destroys its button semantics

`src/components/index-indicator.tsx:71-98`

`role="status"` (an implicit `aria-live="polite"` region) _replaces_ the native
`button` role on a control that has an `onClick`. Screen readers announce it as
a status region and lose "button, 1 of 1", so the pause action is not discoverable
as actionable.

**Fix direction:** keep the `<button>` semantics and put the live region on the
inner `<span>` (`:89-96`), or wrap in a `<div role="status">`.

### M10. Index indicator pauses, then immediately reads back stale state

`src/components/index-indicator.tsx:74-77`

```ts
void indexPause(!paused)
void refresh()
```

`refresh()` is fired without awaiting `indexPause`, so the poll typically
returns pre-toggle status and the button flips back. The next poll (4 s later)
corrects it, giving a visible flicker.

**Fix direction:** `await indexPause(...)` before `refresh()`, or optimistically
`setStatus` with the flipped value.

---

## Low

### L1. Production pays for the debug panel on every single invoke

`src/lib/tauri.ts:10-42`, `src/lib/debug-events.ts:66-99`

`trackedInvoke` runs unconditionally in production: `crypto.randomUUID()`,
`serializeDebugValue(result)` (which does `JSON.parse(JSON.stringify(value))`
for any object/array, e.g. the 60-row `list_recent` payload), a duration read,
and a `CustomEvent` dispatch — all for a listener that only exists when
`import.meta.env.DEV`. It sits directly on the thumbnail hot path (6 concurrent
`getThumbnail` calls per batch).

The `DebugPanel` component itself _is_ tree-shaken out of the production bundle
(verified: no `ctui-debug-panel` strings in `dist/assets/*.js`), so this is
overhead on the caller side only.

**Fix direction:** early-return from `trackedInvoke`'s instrumentation when
`!import.meta.env.DEV`.

### L2. Thumbnail cache grows unbounded and is never pruned

`src/lib/use-thumbnails.ts:17`, `:48`

`thumbs` is a `Record<path, dataURL>` that only ever merges. History (60) plus
search results (60) of 320 px PNG data URLs is on the order of several MB of
base64 held for the process lifetime, including entries whose screenshots have
since been deleted or whose tab was cleared ("Clear view" intentionally keeps
them, per `capture.tsx:565-568`).

**Fix direction:** acceptable as-is for 120 tiles; worth an eviction (LRU or
per-key `URL`-style release) if the limit ever rises or thumbnails get larger.

**Disk half addressed.** The unbounded part that *mattered* — the on-disk cache
at `~/.cache/xreenshoty/thumbs`, one file per `(path, size, mtime, len)` with no
expiry and no way for a user to reclaim it — now has a manual escape hatch:
Settings → Cache → Clear (`clear_thumbnail_cache` + `clearMemoryCaches`). The
in-memory LRU cap (`MAX_CACHED = 120`) is unchanged. Still no *automatic*
eviction of orphaned entries from deleted screenshots; that would need an mtime
or count cap, and is the remaining work here.

### L3. `usePreview()` returns a new `hide` on every render

`src/components/shot.tsx:16-19`

`hide: () => setItem(null)` is a fresh identity each render, and `ShotPreview`
lists `onClose` in its keydown effect deps (`:64`). The Escape listener is
therefore torn down and re-added on every render of the parent — and every
parent render re-renders both `Capture` and `Search` (settings state lives in
`App`).

**Fix direction:** `useCallback` for `hide`, or drop `onClose` from the dep list
via a ref.

### L4. `ShotCard` is not memoised and receives a new callback identity per render

`src/components/shot.tsx:151-214`, `capture.tsx:585-597`, `search.tsx:165-173`

Every card gets an inline `onOpenFolder={() => openFolder(dirArg)}` arrow, so
all 60 cards re-render on any parent state change (status banner, countdown
tick, hover-driven state). The countdown updates state once per second
(`capture.tsx:227`) while a 60-tile grid is mounted.

**Fix direction:** `React.memo(ShotCard)` plus a stable `onOpenFolder`
(`useCallback` in the parent) — or pass `dirArg` down and let the card build the
handler internally.

### L5. Save-folder typing writes localStorage and re-renders the whole tree per keystroke

`src/lib/settings.ts:39-45`, `src/App.tsx:13`

`useSettings` persists on every state change. Typing a path in the save-folder
Input (`capture.tsx:531`, `settings-panel.tsx:142`) therefore performs a
synchronous `localStorage.setItem` and re-renders `App` → `Capture` + `Search` +
`SettingsPanel` for each character.

**Fix direction:** debounce the persistence effect (settings stay in React state
immediately; only the write is deferred).

### L6. History refresh can interleave and land out of order

`src/components/capture.tsx:90-96`, `:133-140`, `:245`

`refresh()` is fire-and-forget from three places: bootstrap import, post-capture,
and (indirectly) save-folder edits. Two overlapping `listRecent()` calls can
resolve in either order, so an older, shorter list may overwrite a newer one
until the next refresh.

**Fix direction:** a monotonic sequence number or an `AbortController`-style
guard so only the latest `listRecent` commits.

### L7. `emitDebugEvent` uses a leftover event namespace from another project

`src/lib/debug-events.ts:1`, `src/components/external-link-guard.tsx:5`,
`src/components/debug-panel.tsx:199-201`, `:1014`

`"ctui:debug"`, `ctui-debug-panel-side`, `ctui://debug-panel-mounted` — while
everything else in the app is namespaced `xreenshoty:` (see
`region-overlay.tsx:15-16`). Purely cosmetic, but it will confuse anyone
grepping for the app prefix.

### L8. Stale project metadata in the shell — **Fixed**

- `index.html:5` — `<title>vite-app</title>` (the Tauri window title comes from
  `tauri.conf.json`, but a browser/preview tab still shows "vite-app").
- `index.html:4` — favicon is `/vite.svg`, the Vite template placeholder.
- `App.tsx:28` — `<Badge>Milestone 3</Badge>` while `plan.md` / `AGENTS.md` are
  well past milestone 5.

**Fix:** `<title>` is `xreenshoty`; the badge reads "Milestone 5" (per `plan.md`,
1–4 and 5.0–5.6 are implemented); favicon is now `public/logo.svg`, copied from
`src-tauri/icons/32x32.png` (567 B). The unused `public/vite.svg` placeholder was
deleted — nothing else referenced it.

### L9. Duplicated `fileName` helper — **Fixed**

`src/components/capture.tsx:64-66` and `src/components/shot.tsx:7-9` are
identical, and `capture.tsx` imports from `shot.tsx` anyway. Export once.

**Fix:** deleted the local copy; `capture.tsx` now takes `fileName` from the
existing `@/components/shot` import.

### L10. Truncated comment left in the source — **Fixed**

`src/components/capture.tsx:143`

```ts
// Thumbnails are loaded in a small worker pool rather than all at once. Each
```

The sentence ends mid-word — the rest of the comment was lost in an edit.

**Fix:** deleted rather than reconstructed. The rationale it was reaching for
already exists in full at `use-thumbnails.ts:5-7` (`CONCURRENCY = 6` and why
requests are pooled), so a rewritten copy would have been a second, weaker
version of it.

### L11. Dev-only tooling listed as production dependencies

`package.json`

- `shadcn` (a CLI/generator) is in `dependencies`.
- `eslint-plugin-tailwind-canonical-classes` (lint plugin) is in `dependencies`.
- `@tailwindcss/node` is in `dependencies` (the Vite plugin is what's needed at
  build time; this belongs in `devDependencies`).

None of these are bundled, but they are installed for any production install.

### L12. Bare `d` key silently toggles the theme

`src/components/theme-provider.tsx:141-179`

An undocumented single-key shortcut with no modifier, no UI hint and no way to
disable. It is correctly guarded against editable targets and key repeat, so it
will not eat typing — but it is undiscoverable and can surprise someone who
presses `d` expecting nothing (e.g. after Escape-closing a dialog).

**Fix direction:** either surface it in Settings alongside the capture
shortcuts, or require a modifier.

### L13. `setTheme` writes localStorage without a try/catch

`src/components/theme-provider.tsx:97`

`localStorage.setItem(storageKey, nextTheme)` is unguarded, unlike the
deliberately guarded write in `settings.ts:39-45` (which documents "private
mode — keep in memory only"). In a storage-restricted context this throws out of
the click handler.

### L14. Index poll never pauses when the window is hidden

`src/components/index-indicator.tsx:53`

`setInterval(…, 4000)` runs for the app's whole lifetime, including when the
window is minimized or on another workspace. Tauri webviews do throttle timers
to a degree, but gating on `document.visibilityState` (or the Tauri window
focus event) would remove 100% of the idle load.

### L15. Number inputs reset to 0 when cleared

`src/components/capture.tsx:506-508`, `settings-panel.tsx:117`

`Number(e.target.value) || 0` means deleting the field to type `15` forces
through `0` first. `update()` clamps correctly, so this is a usability nit, not
a correctness bug — but it makes the field feel broken when replacing a value.

### L16. Search results are not refreshed after a capture

`src/components/search.tsx` vs `capture.tsx:90-96`

`refresh()` only updates `Capture`'s history. A screenshot taken while the
Search tab has results on screen is missing from them until the query changes.
Both tabs are `keepMounted` (`App.tsx:55-67`), so this is visible in normal use.

**Fix direction:** lift history (or a refresh signal) into `App`, or have
`Search` re-run its last query when the index changes.

---

## Checked and _not_ a problem

Recording these so they are not re-investigated:

- **"Multiple words widen the search"** is accurate copy. `db::fts_query`
  (`src-tauri/src/db.rs:602`) joins sanitised terms with `OR`, so the UI text
  matches the actual FTS5 semantics.
- **DebugPanel is genuinely tree-shaken.** `main.tsx:14` guards it with
  `import.meta.env.DEV`; no `ctui-debug-panel` strings appear in
  `dist/assets/*.js` after `bun run build`.
- **`typecheck` is `tsc -b`** and therefore really checks files (AGENTS.md
  trap 4) — it and `eslint` both pass clean.
- **Search debounce has no out-of-order race.** The `cancelled` flag in
  `search.tsx:49` plus `clearTimeout` in cleanup correctly supersede in-flight
  requests; the mode/staleness problems are H2/H3, not races.
- **Tauri event listeners are all cleaned up.** `capture.tsx:153-175` handles
  the async-registration case (`disposed` → unlisten immediately),
  `index-indicator.tsx:56-63` and `region-overlay.tsx:40-49` handle the
  resolved-promise case.
- **`keepMounted` on Capture/Search is correct and intentional** — it preserves
  the thumbnail cache and avoids re-running `import_existing` on every tab
  switch.
- **`dirArg` is read before its `const` declaration** inside
  `applyRegionResult` (`capture.tsx:187` vs `:271`), but the function is only
  ever invoked asynchronously from an effect-registered listener, so the TDZ is
  never hit.
- **The region overlay deliberately omits `index.css`** (`src/region.tsx`) and
  keeps geometry in inline styles — documented and correct per AGENTS.md.

---

# Backend (`src-tauri/`)

Same rules as above: findings only. BH1 and BH2 have since been fixed; both are
marked below.

---

## High

### BH1. The indexer stalls completely whenever the embedding server is unavailable — OCR included — **Fixed**

`src-tauri/src/db.rs:470-498`, `:544-553`; `src-tauri/src/indexer.rs:227-230`, `:286-288`

Three facts combine:

1. `pending(limit)` selects `WHERE needs_ocr OR needs_embedding … ORDER BY
created_at ASC LIMIT 25` — oldest first, **no preference for rows that only
   need OCR**.
2. `mark_embedding_skipped(id)` sets `embed_model = 'skipped'` and leaves
   `embedded_at` NULL. `pending()` keys `needs_embedding` off
   `embedded_at IS NULL`, so **skipped rows are still pending and are returned
   again, forever** (see BH-M2: the doc comment on that function claims the
   opposite).
3. `should_chain()` requires `touched.len() >= BATCH`. With the embedder down,
   `to_embed` is non-empty so the embedding branch only stamps skips, and OCR
   contributes nothing because `needs_ocr` is already false for those rows —
   so `touched == 0` and the loop never chains.

Trace on a fresh 370-row library with no llama-server (missing files, spawn
failure, or the 5-minute `wait_ready` timeout):

- Pass 1: oldest 25 rows are OCR'd → `touched = 25` → chain.
- Pass 2: those same 25 rows are returned again (they still need embedding).
  OCR is a no-op, embedding is skipped → `touched = 0` → **no chain**, sleep
  `IDLE_POLL` (20 s).
- Pass 3…n: identical. The queue head never advances.

**Rows 26–370 are never OCR'd.** Keyword search covers 25 screenshots and the
UI sits on "Loading model…" indefinitely. `ocr.rs` needs no GPU at all, but it
is blocked behind an unrelated subsystem.

This is the common case for any machine that does not have
`~/apps/llama-b11541/` installed.

**Fix:** three changes, all at the point where the two subsystems were coupled.

1. **`db::pending()` now orders `needs_ocr DESC, created_at ASC`** (`db.rs:486`).
   This is the actual root cause and the one that matters: rows owing text can
   no longer sit behind rows the embedding server cannot drain. The backfill
   order within each group is unchanged.
2. **The indexer counts any successful `set_ocr` as progress**, not only the
   non-empty-text case (`indexer.rs:210`). `ocr_boxes` is written either way, so
   the row will not be offered again — a batch of text-free screenshots
   (74 of 370 here) was reporting zero progress and stalling the chain.
3. **`mark_embedding_skipped` no longer re-stamps rows it already skipped**
   (`AND embed_model IS NOT 'skipped'`), and its comment now describes what it
   really does. It deliberately leaves `embedded_at` NULL so the row stays
   pending and is retried once a server exists — treating "skipped" as "done"
   would strand every screenshot captured before the model was installed, which
   is worse than the re-poll it was meant to prevent.

`should_chain` was left alone: it already does the right thing once `touched`
means what it says.

Covered by `db::tests::pending_puts_rows_that_need_ocr_first`, verified to fail
against the old `ORDER BY created_at ASC` and to assert that a skipped row stays
pending.

### BH2. Capture filenames have 1-second resolution, so a second capture silently overwrites the first — **Fixed**

`src-tauri/src/screenshot.rs:46-48`, `:97-112`

```rust
fn timestamp() -> String {
    Local::now().format("%Y-%m-%d-%H-%M-%S").to_string()
}
// ...
let filename = format!("xreenshoty-{}-{}.png", timestamp(), sanitize(stem));
```

`save_png` writes to that path with `image.save(&path)` and never checks for an
existing file. The stems are highly repeatable:

| mode                     | stem                     | collision likelihood                      |
| ------------------------ | ------------------------ | ----------------------------------------- |
| active window            | `active-window`          | constant — two presses in the same second |
| fullscreen (one monitor) | `display-<id>`           | constant                                  |
| region                   | `region-{x}x{y}-{w}x{h}` | constant for an identical drag            |

Delay defaults to 0 (`settings.ts:15`), so nothing throttles rapid captures.
Two captures in the same second produce one file: the second overwrites the
first, `db::record`'s `ON CONFLICT(path) DO UPDATE` updates only
`width`/`height`, and the frontend history shows a single tile. **A screenshot
is lost with no error, no log line, and no UI indication.**

**Fix:** `timestamp()` now emits `%3f` (milliseconds) **and** `save_png` routes
through a new `unique_path(dir, base)`, which suffixes `-2`, `-3`, … until it
can claim a name. Milliseconds alone is not sufficient — two captures inside the
same millisecond still collide — so the suffix is what actually closes the data
loss, and the timestamp change only makes it rare.

`unique_path` reserves with `OpenOptions::new().write(true).create_new(true)`
rather than probing with `exists()`: `save_png` is reachable concurrently
through the global shortcuts, so a check-then-write would keep a TOCTOU window.
The reserved file is then overwritten by `image.save()` immediately after.

Covered by `screenshot::thumbnail_tests::unique_path_suffixes_on_collision`.

### BH3. The llama-server child is never killed on exit — contradicting AGENTS.md — **Fixed**

`src-tauri/src/embedder.rs:131-154`, `:180-229`

`Embedder` holds `child: Mutex<Option<Child>>`. There is **no `impl Drop`
anywhere in `src-tauri/src`** (verified by grep), and Rust does not reap or
terminate children on drop. On app exit the `llama-server` is orphaned and
reparented to init, keeping an ~825 MB model plus its GPU allocation resident.

AGENTS.md states: _"a `Drop` impl kills the child so an 825 MB model is never
left resident."_ That is not implemented.

Knock-on effect on the next launch: `Embedder::start()` sees `/health`
answering and takes the early-return path (`embedder.rs:161-173`), logging
_"reusing an already-running llama-server … its GPU is whatever it was started
with"_ — which is true, but the "somebody else" is the previous run of this
same app.

**Fix:** `Embedder::shutdown()` takes the child, `kill()`s and `wait()`s it.
`child` is only ever `Some` when _this_ process spawned it, so a reused server is
never killed — the "reuse somebody else's llama-server" case is untouched.

`Drop` alone would **not** have fixed this, and that is the interesting part:
the `Embedder` is held by a process-lifetime `OnceLock<Arc<Embedder>>`, so the
last `Arc` never drops and `drop` never runs. Hence the explicit
`indexer::shutdown()` call from `lib.rs`, placed after `tauri::Builder::run`
returns — the one point guaranteed to execute exactly once at exit. `impl Drop`
is kept as a backstop for the normal (non-static) case.

Covered by `embedder::tests::shutdown_kills_the_server_we_spawned`. The
assertion is on _timing_, not on the pid: with a working `kill()`, `wait()`
reaps the child immediately, so shutdown returns in well under a second;
without it, `wait()` blocks until the stand-in process exits on its own. A plain
"is the pid gone" check passes either way, just 20 seconds later — verified by
removing the `kill()` and watching the test still pass.

AGENTS.md's claim is now true.

---

## Medium

### BM1. Heavy work runs on the main thread (Tauri sync commands)

Tauri runs non-`async` commands on the main thread unless marked
`#[tauri::command(async)]`. These are all synchronous:

| command                                              | file:line               | blocking work                                  |
| ---------------------------------------------------- | ----------------------- | ---------------------------------------------- |
| `capture_fullscreen`                                 | `screenshot.rs:128-160` | 1–N monitor grabs + PNG encode + disk write    |
| `capture_region`                                     | `screenshot.rs:216-228` | grab + encode + write                          |
| `capture_active_window`                              | `screenshot.rs:299-311` | X11 round trips + grab + encode                |
| `backdrop_data_url`                                  | `region.rs:29-36`       | full-monitor PNG encode + base64 of the result |
| `save_region_from_backdrop`                          | `region.rs:120-158`     | crop + PNG encode + write                      |
| `open_folder` / `list_monitors` / `default_save_dir` | `screenshot.rs`         | smaller, but still main thread                 |

`backdrop_data_url` is the worst offender: a 1920×1080 RGBA frame is 8 MB
before encoding, and the base64 data URL handed to the overlay is ~10–15 MB.
At 4K that is ~33 MB → ~45 MB of base64, all built on the main thread while
the window is expected to redraw.

**Fix direction:** mark these `#[tauri::command(async)]`, and move the actual
encode into `tauri::async_runtime::spawn_blocking`.

### BM2. Async commands block tokio workers instead of using `spawn_blocking`

The inverse problem: these are `async` but contain no `.await` in their hot
path, so they hold a runtime worker for their whole duration.

- `thumbnail` (`screenshot.rs:406-441`) — full PNG decode + resize + PNG
  encode + base64. The frontend fires **6 concurrently**
  (`use-thumbnails.ts:8`), which can occupy six workers at once.
- `import_existing` (`db.rs:250-310`) — directory scan plus one `open()` +
  24-byte read per file, under the global connection mutex.
- `search_similar` (`db.rs:652-690`) — `embed_query()` is a synchronous HTTP
  call (up to the 120 s `embed` agent timeout).
- `index_status` (`indexer.rs:70-80`) — calls `embedder.is_ready()`, a
  blocking HTTP health check with an 800 ms timeout, on **every 4-second poll
  from the UI**.
- The whole of `Indexer::run` (`indexer.rs:143-291`): `std::thread::scope`
  for OCR (`:190-203`), `std::fs::read` + synchronous HTTP in `embed_one`
  (`:294-309`). A batch of 25 images at ~1.4 s each is ~35 s of one worker
  blocked with no `.await`.

On a many-core machine this is survivable; on a 2-core box (2 workers) the UI
can stall behind it.

**Fix direction:** `tauri::async_runtime::spawn_blocking` for the CPU-bound
bodies, and `.await` around anything that talks to the network or to SQLite.

### BM3. `wait_ready` sleeps on a tokio worker for up to five minutes — **Fixed**

`src-tauri/src/indexer.rs:120-127`, `src-tauri/src/embedder.rs:240-249`

```rust
tauri::async_runtime::spawn(async move {
    if warm.wait_ready(Duration::from_secs(300)) { /* ... */ }
});
```

`wait_ready` is entirely synchronous — `std::thread::sleep(2s)` in a loop —
and the async block never awaits. It therefore occupies one runtime worker
continuously for up to 300 seconds. Every one of those 2-second sleeps is a
blocking call made from inside an async task.

**Fix:** the warm-up task is now `tauri::async_runtime::spawn_blocking`
(`indexer.rs:135`) instead of `spawn`. `wait_ready` is a synchronous poll built
on `thread::sleep`, so on the async pool it held a runtime worker for the whole
five minutes. `spawn_blocking` keeps `wait_ready`'s signature and still leaves
app startup unblocked.

### BM4. `mark_embedding_skipped` does not do what its comment says — **Fixed**

`src-tauri/src/db.rs:542-553`

> _"Marked done even when embedding was skipped, so OCR-only rows stop being
> retried forever while the model server is unavailable."_

The statement only touches `embed_model`. Nothing in `pending()` reads that
column (`db.rs:476`: `(embedded_at IS NULL) AS needs_embedding`), so the row
stays pending. The promised behaviour is not implemented — and is the direct
cause of BH1.

**Fix:** the comment was wrong, not the code — and the wrongness was worth
keeping. A skipped row _must_ stay pending, because "skipped" means "could not
try yet", not "finished": if it stopped being retried, every screenshot
captured before the model was installed would stay unembedded forever, even
after the user starts a server.

So the comment now says that, and the function no longer rewrites `embed_model`
for rows already marked (which was pure churn — an UPDATE per row per idle
poll). Asserted in `pending_puts_rows_that_need_ocr_first`.

The retry-every-poll cost for genuinely _failing_ rows (corrupt PNG, server
errors) is still open — it needs an attempt counter and backoff, which is more
than this fix should carry.

### BM5. `~` is never expanded in the save folder

`src-tauri/src/screenshot.rs:26-36`

```rust
let trimmed = dir.trim();
if trimmed.is_empty() { return Err(...); }
Ok(PathBuf::from(trimmed))
```

`resolve_dir` does a raw `PathBuf::from`, with no tilde or `$VAR` expansion
anywhere in `src-tauri` (verified by grep — the only `home_dir()` use is
`Config::discover`). The frontend placeholder is literally `~/Pictures`
(`capture.tsx:532`, `settings-panel.tsx:143`).

Typing `~/Pictures` therefore makes `ensure_dir` call
`create_dir_all("~/Pictures")`, which resolves **relative to the process CWD**
and creates a directory named `~` — `$HOME/~/Pictures` when launched from a
`.desktop` file, or `./~/Pictures` otherwise. Screenshots land in a nested
folder the user did not ask for, and the status line cheerfully reports
`Saved ~/Pictures/...`.

**Fix direction:** expand a leading `~/` against `dirs::home_dir()` (and
optionally `~user/`, though that is rarely needed).

### BM7. A file dropped into the save folder from outside was never indexed — **Fixed**

Two independent gates rejected it.

1. `import_existing` filtered on `extension() == Some("png")`, so anything a
   browser, clipboard manager or phone saved — `.jpg`, `.jpeg`, `.webp`,
   `.gif` — was skipped outright.
2. Widening (1) alone changed nothing. `png_dimensions` hand-parsed a PNG IHDR
   and returned `None` for everything else, so a JPEG still fell through the
   dimension check and was dropped before the insert.

Either way the file was invisible in history and unsearchable. Because nothing
watched the folder, the only reconciliation ran on launch and on save-folder
change, so a paste mid-session waited for a restart.

**Fix:**

- `is_indexable_image` accepts png/jpg/jpeg/webp/gif, case-insensitively.
  `avif`, `tiff` and `bmp` stay out on purpose — `image` 0.24's default features
  cannot decode them, and a row whose thumbnail always fails is worse than no
  row.
- `png_dimensions` became `image_dimensions`: PNG is still hand-parsed (it is
  what `save_png` writes), everything else falls back to
  `image::guess_format`. Header-only, so the launch scan stays cheap.
- New `watch.rs` holds a non-recursive inotify watch on the Pictures folder and
  runs `import_existing` + `indexer::poke()` once the folder goes quiet
  (`SETTLE` = 400 ms), coalescing the several writes a clipboard save performs
  into one directory scan.

Two limits remain, both structural rather than oversight:

- **Non-recursive**, matching the save folder's own behaviour. A watched folder's
  subfolders are not scanned.
- A file that **overwrites an existing path** with new pixels keeps its old row:
  `ON CONFLICT(path) DO NOTHING` skips it, and `pending()` sees a non-NULL
  `ocr_boxes`/`embedded_at`, so the new content is never OCR'd or embedded.
  Multiple watched folders make this likelier — the same image reachable from
  two of them. Deleting the row and re-importing is the only recovery.

Covered by `db::tests::indexable_extensions_cover_what_clipboards_produce`,
`db::tests::import_indexes_a_pasted_jpeg_but_not_an_avif` (asserts the JPEG's
48x24 dimensions come from its header, so it proves the decode path and not just
the filter), and `watch::tests::settle_window_outlives_a_multistep_write`.

### BM8. Only one folder was ever scanned — **Fixed**

`import_existing(dir)` and the watcher were both single-path, so Downloads or a
shared drive stayed invisible. Settings → **Watched folders** now holds a list
(`settings.scanFolders`) pushed to Rust via `set_scan_folders`, and
`watch::sync(&dirs)` makes the registry match exactly.

Two things that are easy to get wrong here, both now unit-tested:

- **Duplicate watches.** `/x/Pics` and `/x/Pics/` are the same directory and
  inotify accepts both, so every event would fire the reconcile twice. `sync`
  canonicalises its keys before comparing.
- **Empty strings.** An empty path canonicalises to the **CWD**, which would
  silently watch the whole working directory. Blank entries are dropped first.

One bad folder must not cost the others: a path that is not a directory is
logged and skipped, and `set_scan_folders` returns the count actually watched so
the UI can say so.

Covered by `watch::tests::sync_adds_folders_and_drops_removed_ones` and
`watch::tests::sync_skips_folders_that_do_not_exist`.

### BM6. Nothing reconciles the database against the filesystem — **Fixed**

`src-tauri/src/db.rs:250-310` (import only adds), and there is no delete,
prune, or `fs::metadata` re-check anywhere.

Consequences:

- A PNG deleted outside the app keeps its row and its `screenshot_vec`
  entry forever. It still appears in history and in search results; the
  thumbnail command then fails and the card renders the `…` placeholder
  (`shot.tsx:181`) with no indication the file is gone.
- `list_recent` is ordered by `created_at`, so ghosts occupy history slots.
- There is no `delete_screenshot` command at all, so the app cannot even
  offer cleanup.
- The thumbnail cache (`screenshot.rs:377-397`, `~/.cache/xreenshoty/thumbs`)
  grows by one file per (path, size, mtime, len) and is never pruned.

**Fix:** new `db::prune_missing()`, called from `import_existing` — which runs on
launch and, since M1, whenever the save folder changes.

Each row is checked with `Path::exists()` and the missing ones are deleted
inside one transaction. Two details that matter:

- **Existence is checked per row, not by "not in the folder we just imported".**
  Scoping it to the import directory would delete every row from a
  previously-configured save folder the moment you switched to a new one.
- **`vec0` entries are deleted explicitly.** FTS5 cleans itself via the
  `screenshots_fts_ad` trigger, but `vec0` has no triggers — an orphan vector
  survives its screenshot and keeps skewing KNN.

Best-effort: a prune failure logs and does not block the import.

Covered by `db::tests::prune_removes_gone_rows_and_their_search_entries`, which
drives it through `import_existing` (a direct `prune_missing` unit test passes
even when nobody calls it) and asserts the row, its FTS entry and its vector all
disappear, that a surviving file is untouched, and that removing either the vec0
delete or the wiring makes it fail.

Still open from this entry: there is no `delete_screenshot` command, so the app
cannot offer cleanup from its own UI, and the `~/.cache/xreenshoty/thumbs` cache
still grows unbounded.

### BM7. `embed_model` is written but never read, so a model change silently mixes vectors

`src-tauri/src/db.rs:517-540`, `src-tauri/src/embedder.rs:57-61`

`set_embedding` stores `embed_model`, and both its own doc comment and
`model_label()`'s say the purpose is _"so a later model change can be detected
instead of silently mixing incompatible embeddings."_ No code ever queries
that column — not `search_similar` (`db.rs:652-690`), not `index_status`, not
any migration. Swap `XREENSHOTY_EMBED_MODEL` and the index silently
accumulates two incompatible vector spaces in one `vec0` table; KNN then ranks
against whichever vectors happen to be there.

**Fix direction:** compare `embed_model` on read, and re-embed (or partition)
when it differs from the configured model.

### BM8. Port 8080 is assumed to be llama-server

`src-tauri/src/embedder.rs:44-45`, `:160-173`

`port: 8080` is hardcoded and `start()` decides "someone is already running a
server" purely from `GET /health` returning 2xx. Any local service that
answers `/health` with success (a dev server, Jupyter, another llama-server on
a different model) makes `start()` return `Ok` without spawning anything.
Every embedding request then goes to that service, fails to parse, and — via
`indexer.rs:244-248` — stamps rows `skipped` while `server_ready` reads `true`,
so the UI shows "Indexing N of N" never advancing and no error.

**Fix direction:** probe something llama-specific (e.g. `GET /v1/models` and
check the returned id), or make the port configurable and prefer spawning our
own.

### BM9. No timeout on tesseract — **Fixed**

`src-tauri/src/ocr.rs:29-44`

`Command::new("tesseract").output()` waits indefinitely. A wedged
subprocess (huge pathological image, stalled I/O) hangs `ocr::run`, which
blocks the `std::thread::scope` join in `indexer.rs:191-203`, which blocks the
async worker, which stops indexing for the rest of the session. There is no
watchdog anywhere.

**Fix:** two halves, because the timeout alone only changes _how long_ the stall
lasts rather than removing it.

1. **`ocr::run_with_timeout`** spawns and polls `try_wait`, killing the child once
   the deadline passes. No new dependency — `wait-timeout` would work, but
   stdlib polling is enough and this already runs on a dedicated worker thread.
   Limit is `XREENSHOTY_OCR_TIMEOUT_SECS`, default 60s.
2. **A failed row is now marked attempted** (`indexer.rs:227`). Without this the
   timeout converts a permanent hang into a _periodic_ one: `ocr_boxes` stays
   NULL, so `pending()` hands the same hopeless image back every `IDLE_POLL` and
   the worker pays the full timeout again, forever.

The trade-off on (2) is deliberate: a timed-out screenshot loses its text
permanently rather than being retried. That is why the limit sits ~100x above
the real ~0.5s cost rather than being tightened — a false timeout is the only
way this loses data.

Covered by `ocr::tests::a_wedged_process_is_killed_rather_than_waited_on` (a
`sleep 30` given 100ms) plus `a_process_that_finishes_is_left_alone` to guard
the happy path. Also verified end-to-end against the real tesseract 5.5.0 on
`~/Pictures/mx.png`: 0.75s, 6 words with correct coordinates, so the piped
`wait_with_output` path does not silently lose stdout.

### BM10. Resume-from-pause can take up to 20 seconds — **Fixed**

`src-tauri/src/indexer.rs:84-90`, `:144-159`

`index_pause` only stores the flag; it does not poke `wake`. After resume, the
worker is sitting in `tokio::select!` on `wake.notified()` **or**
`sleep(IDLE_POLL)` — both fresh — so it can be up to 20 s before the loop
re-runs and sees `paused == false`.

The `PAUSE_POLL` constant is documented as _"While paused, wake this often so a
resume is felt immediately"_, but the `continue` at `:158` goes back to the
`select!`, so the 400 ms sleep only delays one iteration; it does not make
resume prompt. The comment describes behaviour the code does not have.

This stacks with frontend bug **M10**, which reads status back immediately
after pausing and shows a stale value.

**Fix:** `index_pause` now calls `wake.notify_one()` (`indexer.rs:98`), so both
transitions wake the worker out of `select!` instead of waiting out `IDLE_POLL`.

`PAUSE_POLL` is still what makes a _pause_ cheap — it keeps the loop turning
while paused so the flag is re-checked — but it is not what makes a _resume_
prompt, and its comment was implying otherwise. The behaviour it advertised now
comes from the notify.

Frontend **M10** is still open and stacks with this: it reads status back
without awaiting the pause, so the button can flicker.

---

## Low

### BL1. `xdg-open` children are never reaped

`src-tauri/src/screenshot.rs:317-360`

Both `open_file` and `open_folder` `spawn()` and drop the `Child` immediately.
Rust does not reap on drop, so each invocation whose child exits before the
parent waits leaves a zombie. Clicking "Open folder" a few dozen times
accumulates a few dozen `<defunct>` processes.

**Fix direction:** spawn a detached reaper thread that `wait()`s the child, or
use `pre_exec` + double-fork.

### BL2. The frozen backdrop is never released

`src-tauri/src/region.rs:87`, `:135-157`

`Backdrop` is overwritten on the next `begin_region_select` and cleared only on
the window-build failure path (`:106`). After a successful region save it holds
the full-monitor `RgbaImage` — 8 MB at 1080p, 33 MB at 4K — for the rest of
the session. `close_region_overlay` (`:160-166`) does not touch it.

**Fix direction:** set the slot to `None` at the end of
`save_region_from_backdrop`.

### BL3. A partial multi-monitor capture reports failure but keeps the good files

`src-tauri/src/screenshot.rs:151-159`

`saved.push(save_png(...)?)` — if display 2 fails after display 1 was written,
the command returns `Err`. The frontend's `finishCapture` catch branch
(`capture.tsx:254-255`) sets an error and **skips `refresh()`**, so the
successfully saved screenshots do not appear in history until something else
triggers a reload.

**Fix direction:** return the partial list alongside the error, or have the
frontend refresh on failure too.

### BL4. Region coordinates are only correct at display scale 1

`src-tauri/src/region.rs:89-103` + `src/components/region-overlay.tsx:63-66`

`WebviewWindowBuilder::inner_size(w, h)` takes **logical** pixels while
`display_info.width/height` from the `screenshots` crate are **physical**. The
frontend sends raw `clientX/clientY`, and `save_region_from_backdrop` uses them
directly as physical indices into the backdrop (`region.rs:145-154`). At scale
factor 2 only the top-left quarter of the monitor would be selectable.

Documented as a known limitation in AGENTS.md (_"Fine at scale 1; revisit if
you enable HiDPI"_), recorded here so it is not forgotten when HiDPI is
attempted.

### BL5. Dead API surface on both sides

- `screenshot::capture_region` (`screenshot.rs:216-228`) is registered in the
  invoke handler (`lib.rs:74`) but the frontend's `captureRegion`
  (`src/lib/screenshot.ts:29`) is never called — region capture now goes
  through `save_region_from_backdrop`.
- `db::screenshot_count` (`db.rs:340-346`, registered at `lib.rs:90`) and the
  frontend `screenshotCount` (`screenshot.ts:141`) are both unused.
- `shortcutAvailable` (`src/lib/shortcuts.ts:73`) is exported and never used.

Each is a maintenance cost: `capture_region` in particular still contains a
whole second region-capture path (`capture_virtual_rect`) that can drift from
the backdrop path.

### BL6. `resolve_device()` shells out to `vulkaninfo` during app startup

`src-tauri/src/embedder.rs:80-88`, reached from `Config::discover()` in
`Indexer::start` (`indexer.rs:110`), called from `setup()` (`lib.rs:67`).

`vulkaninfo --summary` is synchronous and typically takes a few hundred
milliseconds. It runs on the main thread inside `setup`, before the main
window is shown (`lib.rs:92-97` keys on `PageLoadEvent::Finished`), so it
delays first paint by that amount on every launch.

**Fix direction:** resolve the device lazily on first embed, or off the main
thread.

### BL7. `index_progress` / `indexed_count` are two queries for the same numbers

`src-tauri/src/db.rs:565-593`

`index_status` calls `index_progress` (3 counts) and the indexer then calls
`indexed_count` (2 counts) at the end of every batch (`indexer.rs:275`) —
`indexed_count` is a strict subset of `index_progress`. Two extra full-table
scans per batch; with the global connection mutex held (`with_conn`), each one
serialises every other DB caller behind it.

### BL8. `post()` error strings can carry the whole response body

`src-tauri/src/embedder.rs:278`

`format!("embedding response had no data: {raw}")` interpolates the complete
HTTP body. A malformed 500 page from a wrong service (see BM8) becomes a
multi-megabyte error string, which is then logged, sent to the frontend as the
command error, and serialized again by `trackedInvoke` (see frontend L1).

### BL9. `image_max_tokens` is documented as 70–1120 but never validated

`src-tauri/src/embedder.rs:46-49`, `:18-21`

`XREENSHOTY_IMAGE_MAX_TOKENS` is parsed with `v.parse().ok().unwrap_or(320)`
and passed straight to llama-server. The doc comment states the model accepts
70–1120; a value outside that range (or `0`) becomes a server-side startup
failure that surfaces only as "embedding server did not become ready in 5
minutes".

---

## Checked and _not_ a problem (backend)

- **FTS5 triggers are correct.** The `ai`/`ad`/`au` triggers on
  `screenshots_fts` (`db.rs:84-106`) handle the external-content table
  properly, including the `WHEN old.ocr_text IS NOT new.ocr_text` guard and the
  `IS NOT NULL AND != ''` conditions on both delete and insert. The schema test
  at `db.rs:374-457` actually exercises insert, update-stale-term, and a real
  vec0 round trip.
- **`fts_query` sanitisation is sound.** Splitting on non-alphanumerics and
  quoting each term leaves no operator reachable; the unit tests at
  `db.rs:699-711` cover the exact inputs that would otherwise be syntax errors.
- **`should_chain`'s anti-spin guard works as designed** for the case it was
  written for (a full batch where _nothing_ was processable). Its test at
  `indexer.rs:317-326` passes. The defect in BH1 is that `touched` is also 0
  for a batch where rows were deliberately skipped — a case the test does not
  cover.
- **vec0 KNN is written correctly**: `v.embedding MATCH ?1 AND v.k = ?2`
  against the table itself, not an aliased join (`db.rs:663-669`), which is the
  trap AGENTS.md documents.
- **`sqlite-vec` registration is before `Connection::open`** and guarded by a
  `Once` (`db.rs:41-48`), with the documented `transmute` bridge.
- **`set_embedding` uses DELETE + INSERT in one transaction**, since `vec0`
  has no UPSERT — and stamps `embedded_at` in the same transaction, so the
  timestamp and the vector cannot disagree.
- **Connection caching is correctly keyed by path**, so `XREENSHOTY_TEST_DB`
  cannot alias onto the real index, and the `with_conn` lock is recovered from
  poisoning rather than panicking.
- **Region failure paths restore the main window.** `begin_region_select`
  shows it again on both the capture error (`region.rs:81-84`) and the
  window-build error (`:104-108`).
- **The OCR "attempted" marker is right.** `set_ocr` always writes
  `ocr_boxes` (`db.rs:504-513`), and `pending()` keys off that column, so
  text-free screenshots are not re-OCR'd forever — the exact trap AGENTS.md
  warns about is avoided.
- **Log filtering is in place** at `lib.rs:48-52`, so the `ureq` TRACE noise
  problem AGENTS.md describes does not recur.
