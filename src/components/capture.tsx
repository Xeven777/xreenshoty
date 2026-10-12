import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { isTauri } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"
import { AnimatePresence, motion } from "motion/react"
import {
  AppWindow,
  Camera,
  Crop,
  FolderOpen,
  Image as ImageIcon,
  Minus,
  Monitor as MonitorIcon,
  Plus,
  Trash,
  Warning,
  X,
} from "@phosphor-icons/react"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  REGION_CANCELLED,
  REGION_SELECTED,
  type RegionRect,
} from "@/components/region-overlay"
import { notify } from "@/lib/notify"
import { MAX_DELAY, type Settings } from "@/lib/settings"
import { bindShortcut, releaseShortcut } from "@/lib/shortcuts"
import {
  toastDeleted,
  toastFailed,
  toastSaved,
  toastSavedMany,
} from "@/lib/toast"
import { useThumbnails } from "@/lib/use-thumbnails"
import { dirOf, fileName } from "@/lib/utils"
import { ShotCard, ShotPreview, usePreview } from "@/components/shot"
import {
  beginRegionSelect,
  captureActiveWindow,
  deleteScreenshot,
  captureFullscreen,
  closeRegionOverlay,
  getDefaultSaveDir,
  importExisting,
  listMonitors,
  listRecent,
  openFolderSafe,
  restoreMainWindow,
  saveRegionFromBackdrop,
  type MonitorInfo,
  type Screenshot,
} from "@/lib/screenshot"

type Mode = "fullscreen" | "region" | "window"
const HISTORY_LIMIT = 60

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

export function Capture({
  settings,
  update,
  onShortcutError,
}: {
  settings: Settings
  update: <K extends keyof Settings>(key: K, value: Settings[K]) => void
  onShortcutError: (message: string | null) => void
}) {
  const tauri = isTauri()
  const [monitors, setMonitors] = useState<MonitorInfo[]>([])
  const [monitorSel, setMonitorSel] = useState("all")
  const [history, setHistory] = useState<Screenshot[]>([])
  const [busy, setBusy] = useState<Mode | null>(null)
  const [countdown, setCountdown] = useState<number | null>(null)
  const abortRef = useRef(false)

  const { saveDir, delaySecs, notifyOnSave } = settings

  // History comes from SQLite, so it survives restarts. Refreshed after every
  // capture rather than patched locally, to keep one source of truth.
  const refresh = useCallback(async () => {
    try {
      setHistory(await listRecent(HISTORY_LIMIT))
    } catch (e) {
      // No retry action: refresh re-runs on every capture/import anyway.
      toastFailed("Could not load history", e)
    }
  }, [])

  const seededSaveDir = useRef(false)
  useEffect(() => {
    if (!tauri || saveDir || seededSaveDir.current) return
    seededSaveDir.current = true
    getDefaultSaveDir()
      .then((def) => update("saveDir", def))
      .catch(() => undefined)
  }, [tauri, saveDir, update])

  useEffect(() => {
    if (!tauri) return
    let cancelled = false
    listMonitors()
      .then((mons) => {
        if (cancelled) return
        setMonitors(mons)
      })
      .catch((e) => {
        if (!cancelled) toastFailed("Monitor lookup failed", e)
      })
    return () => {
      cancelled = true
    }
  }, [tauri])

  // Index PNGs already sitting in the save folder, then show history. Import is
  // idempotent, so this is safe to re-run. Keyed on the last folder handled, not
  // on a "have we ever booted" boolean — otherwise editing the save folder left
  // history pointing at the old directory until some later capture happened to
  // call `refresh()`.
  const importedDir = useRef<string | null>(null)
  useEffect(() => {
    if (!tauri || !saveDir) return
    const dir = saveDir.trim()
    if (importedDir.current === dir) return
    importedDir.current = dir
    void (async () => {
      try {
        await importExisting(dir || null)
      } catch {
        /* import is a convenience; a failure must not block startup */
      }
      await refresh()
    })()
  }, [tauri, saveDir, refresh])

  const preview = usePreview()
  const thumbs = useThumbnails(
    useMemo(() => history.map((h) => h.path), [history])
  )

  /** Removes the PNG from disk and its row from the index, then drops it from
   *  the visible list. The file goes last: if the DB write fails the picture is
   *  still there. */
  const removeItem = useCallback((item: Screenshot) => {
    deleteScreenshot(item.path)
      .then(() => {
        setHistory((prev) => prev.filter((h) => h.path !== item.path))
        toastDeleted(item.path)
      })
      .catch((e: unknown) => toastFailed("Delete failed", e))
  }, [])

  const openFolderOf = useCallback((item: Screenshot) => {
    void openFolderSafe(dirOf(item.path))
  }, [])

  useEffect(() => () => void (abortRef.current = true), [])

  // The overlay reports its result as an event. The handler is read through a
  // ref so the listener can stay registered while still using fresh state.
  const regionResultRef = useRef<(rect: RegionRect | null) => void>(() => {})

  useEffect(() => {
    if (!tauri) return
    const subs: UnlistenFn[] = []
    let disposed = false
    void (async () => {
      const [onPicked, onCancelled] = await Promise.all([
        listen<RegionRect | null>(REGION_SELECTED, (event) =>
          regionResultRef.current(event.payload ?? null)
        ),
        listen(REGION_CANCELLED, () => regionResultRef.current(null)),
      ])
      if (disposed) {
        onPicked()
        onCancelled()
        return
      }
      subs.push(onPicked, onCancelled)
    })()
    return () => {
      disposed = true
      for (const un of subs) un()
    }
  }, [tauri])

  async function applyRegionResult(rect: RegionRect | null) {
    try {
      if (rect && rect.width > 0 && rect.height > 0) {
        // Crops the frozen backdrop, so no live screen grab happens here.
        await finishCapture(() =>
          saveRegionFromBackdrop(
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            dirArg
          )
        )
      }
    } finally {
      await closeRegionOverlay().catch(() => undefined)
      await restoreMainWindow().catch(() => undefined)
      resetCapture()
    }
  }

  async function startRegionSelect() {
    if (!tauri || busy) return
    abortRef.current = false
    setBusy("region")
    try {
      if (!(await runCountdown())) return
      // From here the overlay owns the interaction; its event finishes the job.
      await beginRegionSelect()
    } catch (e) {
      const cancelled = abortRef.current
      resetCapture()
      if (!cancelled) toastFailed("Region capture failed", e)
    }
  }

  useEffect(() => {
    regionResultRef.current = (rect) => void applyRegionResult(rect)
  })

  function resetCapture() {
    setBusy(null)
    setCountdown(null)
    abortRef.current = false
  }

  /** Abandon whatever is in flight. During the countdown the loop polls
   *  `abortRef` and unwinds itself; once the overlay owns the screen nothing
   *  else will, so tear it down here — otherwise the app sits with every
   *  capture control disabled and no visible way out (H1). */
  async function cancelCapture() {
    abortRef.current = true
    if (busy === "region" && countdown === null) {
      await closeRegionOverlay().catch(() => undefined)
      await restoreMainWindow().catch(() => undefined)
      resetCapture()
    }
  }

  /** Cancellable countdown. Returns false when the user cancelled. */
  async function runCountdown() {
    for (let s = delaySecs; s > 0; s--) {
      setCountdown(s)
      await sleep(1000)
      if (abortRef.current) {
        resetCapture()
        return false
      }
    }
    setCountdown(null)
    return true
  }

  /** Runs the capture itself and records the result. No delay handling. */
  async function finishCapture(fn: () => Promise<string | string[]>) {
    if (!tauri) return
    try {
      const result = await fn()
      if (abortRef.current) return
      const paths = Array.isArray(result) ? result : [result]
      await refresh()
      if (paths.length === 1) {
        toastSaved(paths[0], () => void openFolderSafe(dirOf(paths[0])))
      } else {
        toastSavedMany(paths.length, paths[0])
      }
      if (notifyOnSave) void notify("Screenshot saved", fileName(paths[0]))
    } catch (e) {
      if (!abortRef.current) toastFailed("Capture failed", e)
    } finally {
      resetCapture()
    }
  }

  /** Delay + cancellable countdown, then run the capture. */
  async function runCapture(mode: Mode, fn: () => Promise<string | string[]>) {
    if (!tauri || busy) return
    abortRef.current = false
    setBusy(mode)
    if (!(await runCountdown())) return
    await finishCapture(fn)
  }

  const dirArg = saveDir.trim() ? saveDir.trim() : null
  const capturing = busy !== null

  const doFullscreen = () => {
    const id = monitorSel === "all" ? null : Number(monitorSel)
    void runCapture("fullscreen", () => captureFullscreen(id, dirArg))
  }
  const doWindow = () =>
    void runCapture("window", () => captureActiveWindow(dirArg))

  // Global shortcuts reuse the exact same code path as the buttons, so they
  // get the delay, cancel window, history and notifications for free.
  const actionsRef = useRef({
    fullscreen: null as null | (() => void),
    window: null as null | (() => void),
  })
  useEffect(() => {
    actionsRef.current.fullscreen = doFullscreen
    actionsRef.current.window = doWindow
  }, [monitorSel, dirArg, busy, tauri, delaySecs]) // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!tauri) return
    const full = settings.shortcutFullscreen
    const win = settings.shortcutWindow
    let disposed = false

    void (async () => {
      const fullError = await bindShortcut(full, () =>
        actionsRef.current.fullscreen?.()
      )
      if (disposed) return
      const winError =
        win && win !== full
          ? await bindShortcut(win, () => actionsRef.current.window?.())
          : null
      if (disposed) return
      onShortcutError(fullError ?? winError)
    })()

    return () => {
      disposed = true
      void releaseShortcut(full)
      if (win && win !== full) void releaseShortcut(win)
    }
  }, [
    tauri,
    settings.shortcutFullscreen,
    settings.shortcutWindow,
    onShortcutError,
  ])

  const countdownLabel =
    countdown !== null ? `Capturing in ${countdown}…` : "Capture"

  function stepDelay(delta: number) {
    update("delaySecs", Math.min(MAX_DELAY, Math.max(0, delaySecs + delta)))
  }

  return (
    <div className="space-y-4">
      {!tauri ? (
        <div className="flex items-start gap-3 rounded-2xl border border-dashed p-4 text-sm text-muted-foreground">
          <Warning size={18} className="mt-0.5 shrink-0" />
          <p>
            Capture needs the desktop shell. Run{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-xs">
              bun run tauri dev
            </code>
            .
          </p>
        </div>
      ) : null}

      <div className="grid gap-4 md:grid-cols-3">
        <section className="flex flex-col gap-4 rounded-2xl border bg-card p-5 shadow-sm">
          <header className="flex items-center gap-2.5">
            <MonitorIcon size={20} className="text-muted-foreground" />
            <div>
              <h2 className="font-medium">Fullscreen</h2>
              <p className="text-xs text-muted-foreground">
                One monitor or all of them
              </p>
            </div>
          </header>
          <label className="block space-y-1.5 text-xs font-medium text-muted-foreground">
            MONITOR
            <select
              value={monitorSel}
              onChange={(e) => setMonitorSel(e.target.value)}
              disabled={!tauri || capturing}
              className="h-9 w-full rounded-3xl border border-transparent bg-input/50 px-3 text-sm text-foreground outline-none focus-visible:border-ring disabled:opacity-50"
            >
              <option value="all">
                All monitors{monitors.length ? ` (${monitors.length})` : ""}
              </option>
              {monitors.map((m) => (
                <option key={m.id} value={String(m.id)}>
                  Display {m.id} — {m.width}×{m.height}
                  {m.x !== 0 || m.y !== 0 ? ` @ ${m.x},${m.y}` : ""}
                </option>
              ))}
            </select>
          </label>
          <Button
            onClick={doFullscreen}
            disabled={!tauri || capturing}
            className="mt-auto w-full"
          >
            <Camera data-icon="inline-start" />
            {busy === "fullscreen" ? countdownLabel : "Capture"}
          </Button>
        </section>

        <section className="flex flex-col gap-4 rounded-2xl border bg-card p-5 shadow-sm">
          <header className="flex items-center gap-2.5">
            <Crop size={20} className="text-muted-foreground" />
            <div>
              <h2 className="font-medium">Region</h2>
              <p className="text-xs text-muted-foreground">
                Drag to select on screen
              </p>
            </div>
          </header>
          <p className="text-xs leading-relaxed text-muted-foreground">
            The screen freezes, then you drag a box over it. Add a delay first
            if you need to move the window you want to grab.
          </p>
          <Button
            onClick={() => void startRegionSelect()}
            disabled={!tauri || capturing}
            className="mt-auto w-full"
          >
            <Crop data-icon="inline-start" />
            {busy === "region" ? countdownLabel : "Drag to select"}
          </Button>
        </section>

        <section className="flex flex-col gap-4 rounded-2xl border bg-card p-5 shadow-sm">
          <header className="flex items-center gap-2.5">
            <AppWindow size={20} className="text-muted-foreground" />
            <div>
              <h2 className="font-medium">Active window</h2>
              <p className="text-xs text-muted-foreground">
                Borders included, like XFCE −w
              </p>
            </div>
          </header>
          <p className="text-xs leading-relaxed text-muted-foreground">
            Add a delay, hit capture, then focus the window you want before the
            timer runs out.
          </p>
          <div className="rounded-xl bg-muted/60 px-3 py-2 text-xs text-muted-foreground">
            X11 only in this milestone.
          </div>
          <Button
            onClick={doWindow}
            disabled={!tauri || capturing}
            className="mt-auto w-full"
          >
            <AppWindow data-icon="inline-start" />
            {busy === "window" ? countdownLabel : "Capture"}
          </Button>
        </section>
      </div>

      <AnimatePresence initial={false}>
        {capturing && (countdown !== null || busy === "region") ? (
          <motion.div
            key="countdown"
            initial={{ opacity: 0, y: -6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -6 }}
            transition={{ duration: 0.18 }}
            className="flex items-center justify-between gap-4 rounded-2xl border bg-card p-4 shadow-sm"
          >
            <p className="text-sm text-muted-foreground">
              {countdown === null ? (
                <>
                  Region capture waiting for a selection — drag on the overlay,
                  or cancel.
                </>
              ) : (
                <>
                  {busy === "region"
                    ? "Region"
                    : busy === "window"
                      ? "Active window"
                      : "Fullscreen"}{" "}
                  capture starts in{" "}
                  <span className="relative inline-flex w-8 justify-center overflow-hidden font-semibold text-foreground tabular-nums">
                    <AnimatePresence mode="popLayout" initial={false}>
                      <motion.span
                        key={countdown}
                        initial={{ y: 12, opacity: 0 }}
                        animate={{ y: 0, opacity: 1 }}
                        exit={{ y: -12, opacity: 0 }}
                        transition={{ duration: 0.18 }}
                      >
                        {countdown}s
                      </motion.span>
                    </AnimatePresence>
                  </span>
                </>
              )}
            </p>
            <Button
              size="sm"
              variant="destructive"
              onClick={() => {
                void cancelCapture()
              }}
            >
              <X data-icon="inline-start" />
              Cancel
            </Button>
          </motion.div>
        ) : null}
      </AnimatePresence>

      <section className="space-y-3 rounded-2xl border bg-card p-5 shadow-sm">
        <div className="flex flex-wrap items-end gap-6">
          <div className="space-y-1.5">
            <p className="text-xs font-medium text-muted-foreground">DELAY</p>
            <div className="flex items-center gap-2">
              <Button
                size="icon-sm"
                variant="secondary"
                aria-label="Decrease delay"
                disabled={!tauri || capturing || delaySecs <= 0}
                onClick={() => stepDelay(-1)}
              >
                <Minus />
              </Button>
              <Input
                type="number"
                min={0}
                max={MAX_DELAY}
                value={delaySecs}
                disabled={!tauri || capturing}
                onChange={(e) =>
                  update("delaySecs", Number(e.target.value) || 0)
                }
                className="w-16 text-center tabular-nums"
              />
              <span className="text-xs text-muted-foreground">sec</span>
              <Button
                size="icon-sm"
                variant="secondary"
                aria-label="Increase delay"
                disabled={!tauri || capturing || delaySecs >= MAX_DELAY}
                onClick={() => stepDelay(1)}
              >
                <Plus />
              </Button>
            </div>
          </div>

          <div className="min-w-0 flex-1 space-y-1.5">
            <p className="text-xs font-medium text-muted-foreground">
              SAVE FOLDER
            </p>
            <div className="flex gap-2">
              <Input
                value={saveDir}
                onChange={(e) => update("saveDir", e.target.value)}
                placeholder="~/Pictures"
                disabled={!tauri}
                className="ui-selectable"
              />
              <Button
                size="icon"
                variant="secondary"
                title="Open save folder"
                disabled={!tauri}
                onClick={() => void openFolderSafe(dirArg)}
              >
                <FolderOpen />
              </Button>
            </div>
          </div>
        </div>
      </section>

      <section className="space-y-3 rounded-2xl border bg-card p-5 shadow-sm">
        <div className="flex items-center gap-2.5">
          <h2 className="flex-1 font-medium">History</h2>
          {history.length > 0 ? (
            <Badge variant="secondary">{history.length}</Badge>
          ) : null}
          {history.length > 0 ? (
            <Button
              size="xs"
              variant="ghost"
              onClick={() => {
                // Only the list is cleared. The thumbnail cache is keyed by
                // path inside the hook, so it stays valid and re-showing the
                // same screenshot costs nothing.
                setHistory([])
              }}
            >
              <Trash data-icon="inline-start" />
              Clear view
            </Button>
          ) : null}
        </div>

        {history.length === 0 ? (
          <div className="flex items-center gap-3 rounded-xl bg-muted/60 p-4 text-sm text-muted-foreground">
            <ImageIcon size={20} />
            Nothing indexed yet. Captures and PNGs already in your save folder
            show up here.
          </div>
        ) : (
          <div className="shot-grid grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
            {/* AnimatePresence renders no DOM of its own, so the cards stay
                direct grid children while its direct children (the cards) are
                what gets exit-animated. popLayout pops the leaving card out of
                flow instead of leaving a gap. */}
            <AnimatePresence mode="popLayout" initial={false}>
              {history.map((item, i) => (
                <ShotCard
                  key={item.path}
                  item={item}
                  index={i}
                  thumb={thumbs[item.path]}
                  onOpen={preview.show}
                  onDelete={removeItem}
                  onOpenFolder={openFolderOf}
                />
              ))}
            </AnimatePresence>
          </div>
        )}
      </section>

      <ShotPreview
        item={preview.item}
        onClose={preview.hide}
        onOpenFolder={openFolderOf}
      />
    </div>
  )
}
