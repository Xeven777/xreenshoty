import { useEffect, useState } from "react"
import { isTauri } from "@tauri-apps/api/core"
import {
  Bell,
  Broom,
  Check,
  FolderOpen,
  FolderPlus,
  Keyboard,
  Minus,
  Plus,
  Smiley,
  TextT,
  Trash,
  Warning,
  X,
} from "@phosphor-icons/react"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { MAX_DELAY, type Settings } from "@/lib/settings"
import {
  clearThumbnailCache,
  faceStatus,
  getDefaultSaveDir,
  openFolderSafe,
  ocrStatus,
  setScanFolders,
  thumbnailCacheSize,
  type FaceStatus,
  type OcrStatus,
} from "@/lib/screenshot"
import { eventToAccelerator, prettyAccelerator } from "@/lib/shortcuts"
import { clearMemoryCaches } from "@/lib/use-thumbnails"
import { clearFaceThumbnailCache } from "@/lib/use-face-thumbnails"
import { toastCacheCleared, toastFailed, toast } from "@/lib/toast"
import { formatBytes } from "@/lib/utils"
export function SettingsPanel({
  settings,
  update,
  shortcutError,
}: {
  settings: Settings
  update: <K extends keyof Settings>(key: K, value: Settings[K]) => void
  shortcutError: string | null
}) {
  const tauri = isTauri()
  const [recording, setRecording] = useState<
    "shortcutFullscreen" | "shortcutWindow" | null
  >(null)
  const [cacheBytes, setCacheBytes] = useState<number | null>(null)
  const [clearing, setClearing] = useState(false)
  const [newFolder, setNewFolder] = useState("")
  const [face, setFace] = useState<FaceStatus | null>(null)
  const [ocr, setOcr] = useState<OcrStatus | null>(null)

  // The cache size is only read when the tab is mounted, so the number stays
  // honest for as long as Settings is open.
  useEffect(() => {
    if (!tauri) return
    let cancelled = false
    void thumbnailCacheSize()
      .then((bytes) => {
        if (!cancelled) setCacheBytes(bytes)
      })
      .catch(() => undefined)
    return () => {
      cancelled = true
    }
  }, [tauri])

  async function clearCache() {
    setClearing(true)
    try {
      const freed = await clearThumbnailCache()
      // The in-memory pools must go too, or the grids keep serving what they
      // already have and the clear looks like a no-op.
      clearMemoryCaches()
      clearFaceThumbnailCache()
      setCacheBytes(0)
      toastCacheCleared(freed)
    } catch (error) {
      toastFailed("Could not clear cache", error, () => void clearCache())
    } finally {
      setClearing(false)
    }
  }

  const scanFolders = settings.scanFolders

  /** Push the list to Rust on every change: the watcher registry is replaced to
   *  match, so removing a folder really stops the watch rather than just hiding
   *  it from the UI. `saveDir` is deliberately excluded — it is watched as the
   *  capture destination and lives in its own setting. */
  async function applyScanFolders(next: string[]) {
    update("scanFolders", next)
    try {
      const watched = await setScanFolders(next)
      if (watched < next.length) {
        toast.error("A folder was skipped", {
          description: "Only existing directories can be watched.",
        })
      }
    } catch (error) {
      toastFailed("Could not update watched folders", error, () =>
        void applyScanFolders(next)
      )
    }
  }

  function addScanFolder() {
    const dir = newFolder.trim()
    if (!dir) return
    // Deduplicate, or the same path shows twice and Rust canonicalises both to
    // one watch anyway.
    if (scanFolders.includes(dir)) {
      toast.error("Already in the list")
      setNewFolder("")
      return
    }
    void applyScanFolders([...scanFolders, dir])
    setNewFolder("")
  }

  function removeScanFolder(dir: string) {
    void applyScanFolders(scanFolders.filter((d) => d !== dir))
  }

  // The face opt-in itself is pushed from `App`, which is always mounted —
  // pushing it from here only worked if this tab happened to be open. The
  // readiness readout still refreshes when the toggle flips.
  useEffect(() => {
    if (!tauri) return
    void faceStatus()
      .then((s) => setFace(s))
      .catch(() => undefined)
  }, [tauri, settings.faceIndexing])

  // The OCR readiness is read once: nothing here changes it, and the missing
  // piece has to be visible rather than silently disabling text search.
  useEffect(() => {
    if (!tauri) return
    let cancelled = false
    void ocrStatus()
      .then((s) => {
        if (!cancelled) setOcr(s)
      })
      .catch(() => undefined)
    return () => {
      cancelled = true
    }
  }, [tauri])

  // The banner lives in this tab only; a toast reaches the user wherever they
  // are when the bind fails (usually triggered from Capture).
  useEffect(() => {
    if (shortcutError)
      toast.error("Shortcut not registered", {
        description: shortcutError,
      })
  }, [shortcutError])

  useEffect(() => {
    if (!recording) return
    const onKeyDown = (event: KeyboardEvent) => {
      event.preventDefault()
      event.stopPropagation()
      if (event.key === "Escape") {
        setRecording(null)
        return
      }
      const accel = eventToAccelerator(event)
      if (accel) {
        update(recording, accel)
        setRecording(null)
      }
    }
    window.addEventListener("keydown", onKeyDown, true)
    return () => window.removeEventListener("keydown", onKeyDown, true)
  }, [recording, update])

  function recorder(key: "shortcutFullscreen" | "shortcutWindow") {
    const active = recording === key
    return (
      <div className="space-y-1.5">
        <p className="text-xs font-medium text-muted-foreground">
          {key === "shortcutFullscreen"
            ? "FULLSCREEN SHORTCUT"
            : "ACTIVE WINDOW SHORTCUT"}
        </p>
        <Button
          variant={active ? "default" : "secondary"}
          disabled={!tauri}
          onClick={() => setRecording(active ? null : key)}
          className="w-full justify-between font-mono"
        >
          <span className="truncate">
            {active
              ? "Press keys…"
              : prettyAccelerator(settings[key] || "Not set")}
          </span>
          <Keyboard data-icon="inline-end" />
        </Button>
      </div>
    )
  }

  return (
    <div className="space-y-4">
      {shortcutError ? (
        <div className="flex items-start gap-3 rounded-2xl bg-destructive/10 p-4 text-sm">
          <Warning size={18} className="mt-0.5 shrink-0 text-destructive" />
          <p className="min-w-0 flex-1">{shortcutError}</p>
        </div>
      ) : null}

      <section className="space-y-4 rounded-2xl border bg-card p-5 shadow-sm">
        <header>
          <h2 className="font-medium">Capture</h2>
          <p className="text-xs text-muted-foreground">
            Delay and destination for every capture mode
          </p>
        </header>

        <div className="space-y-1.5">
          <p className="text-xs font-medium text-muted-foreground">DELAY</p>
          <div className="flex items-center gap-2">
            <Button
              size="icon-sm"
              variant="secondary"
              aria-label="Decrease delay"
              disabled={!tauri || settings.delaySecs <= 0}
              onClick={() =>
                update("delaySecs", Math.max(0, settings.delaySecs - 1))
              }
            >
              <Minus />
            </Button>
            <Input
              type="number"
              min={0}
              max={MAX_DELAY}
              value={settings.delaySecs}
              disabled={!tauri}
              onChange={(e) => update("delaySecs", Number(e.target.value) || 0)}
              className="w-20 text-center tabular-nums"
            />
            <span className="text-xs text-muted-foreground">sec</span>
            <Button
              size="icon-sm"
              variant="secondary"
              aria-label="Increase delay"
              disabled={!tauri || settings.delaySecs >= MAX_DELAY}
              onClick={() =>
                update("delaySecs", Math.min(MAX_DELAY, settings.delaySecs + 1))
              }
            >
              <Plus />
            </Button>
          </div>
        </div>

        <div className="space-y-1.5">
          <p className="text-xs font-medium text-muted-foreground">
            SAVE FOLDER
          </p>
          <div className="flex gap-2">
            <Input
              value={settings.saveDir}
              onChange={(e) => update("saveDir", e.target.value)}
              placeholder="~/Pictures"
              disabled={!tauri}
              className="ui-selectable"
            />
            <Button
              size="icon"
              variant="secondary"
              title="Open folder"
              disabled={!tauri || !settings.saveDir.trim()}
              onClick={() =>
                void openFolderSafe(settings.saveDir.trim() || null)
              }
            >
              <FolderOpen />
            </Button>
            <Button
              size="sm"
              variant="ghost"
              disabled={!tauri}
              onClick={() =>
                getDefaultSaveDir()
                  .then((def) => update("saveDir", def))
                  .catch(() => undefined)
              }
            >
              Reset
            </Button>
          </div>
          <p className="text-xs text-muted-foreground">
            Leave empty to use the system Pictures folder.
          </p>
        </div>
      </section>

      <section className="space-y-4 rounded-2xl border bg-card p-5 shadow-sm">
        <header>
          <h2 className="font-medium">Global shortcuts</h2>
          <p className="text-xs text-muted-foreground">
            Work from any app. Click a field, then press the key combination.
          </p>
        </header>
        <div className="grid gap-4 sm:grid-cols-2">
          {recorder("shortcutFullscreen")}
          {recorder("shortcutWindow")}
        </div>
        <p className="text-xs text-muted-foreground">
          Escape cancels recording. A modifier is required so typing is never
          hijacked. If a combination is already claimed by XFCE, change it here
          or in the desktop settings.
        </p>
      </section>

      <section className="space-y-4 rounded-2xl border bg-card p-5 shadow-sm">
        <header className="flex items-center gap-2.5">
          <Bell size={18} className="text-muted-foreground" />
          <div>
            <h2 className="font-medium">Feedback</h2>
            <p className="text-xs text-muted-foreground">
              Desktop notification after a successful save
            </p>
          </div>
        </header>
        <div className="flex items-center justify-between gap-4">
          <span className="text-sm">
            {settings.notifyOnSave ? "Enabled" : "Disabled"}
          </span>
          <Button
            size="sm"
            variant={settings.notifyOnSave ? "default" : "secondary"}
            disabled={!tauri}
            onClick={() => update("notifyOnSave", !settings.notifyOnSave)}
          >
            {settings.notifyOnSave ? (
              <Check data-icon="inline-start" />
            ) : (
              <X data-icon="inline-start" />
            )}
            {settings.notifyOnSave ? "On" : "Off"}
          </Button>
        </div>
        {!tauri ? (
          <Badge variant="secondary">Needs the desktop shell</Badge>
        ) : null}
      </section>

      <section className="space-y-4 rounded-2xl border bg-card p-5 shadow-sm">
        <header className="flex items-center gap-2.5">
          <TextT size={18} className="text-muted-foreground" />
          <div>
            <h2 className="font-medium">Text recognition</h2>
            <p className="text-xs text-muted-foreground">
              Reads the text out of new screenshots so they can be searched.
            </p>
          </div>
        </header>
        <div className="flex items-center justify-between gap-4">
          <span className="text-sm">
            {ocr
              ? ocr.venv_ready && ocr.models_present
                ? "Ready"
                : "Not installed"
              : "—"}
          </span>
          <Badge variant="secondary">{ocr?.engine ?? "OCR"}</Badge>
        </div>
        <p className="text-xs text-muted-foreground">
          Runs on this machine and never leaves it. Needs the ocr-env venv and
          the model files under ~/apps/models/indra-ocr — run
          scripts/setup-ocr.sh once.
          {ocr
            ? ` venv ${ocr.venv_ready ? "ready" : "missing"} · models ${ocr.models_present ? "present" : "missing"}.`
            : ""}
        </p>
      </section>

      <section className="space-y-4 rounded-2xl border bg-card p-5 shadow-sm">
        <header className="flex items-center gap-2.5">
          <Smiley size={18} className="text-muted-foreground" />
          <div>
            <h2 className="font-medium">Face indexing</h2>
            <p className="text-xs text-muted-foreground">
              Group screenshots by who appears in them. Off by default.
            </p>
          </div>
        </header>
        <div className="flex items-center justify-between gap-4">
          <span className="text-sm">
            {settings.faceIndexing ? "Enabled" : "Disabled"}
          </span>
          <Button
            size="sm"
            variant={settings.faceIndexing ? "default" : "secondary"}
            disabled={!tauri}
            onClick={() => update("faceIndexing", !settings.faceIndexing)}
          >
            {settings.faceIndexing ? (
              <Check data-icon="inline-start" />
            ) : (
              <X data-icon="inline-start" />
            )}
            {settings.faceIndexing ? "On" : "Off"}
          </Button>
        </div>
        <p className="text-xs text-muted-foreground">
          Faces are detected on this machine and never leave it, but the index
          then holds biometric templates — turn this on only if you want that.
          Needs the face models under ~/apps/models plus the face-env venv.
          {face
            ? ` ${face.face_count} face(s) indexed · venv ${face.venv_ready ? "ready" : "missing"} · models ${face.models_present ? "present" : "missing"}.`
            : ""}
        </p>
        {!tauri ? (
          <Badge variant="secondary">Needs the desktop shell</Badge>
        ) : null}
      </section>

      <section className="space-y-4 rounded-2xl border bg-card p-5 shadow-sm">
        <header className="flex items-center gap-2.5">
          <Broom size={18} className="text-muted-foreground" />
          <div>
            <h2 className="font-medium">Cache</h2>
            <p className="text-xs text-muted-foreground">
              Downscaled copies of your screenshots, kept so scrolling the grid
              stays instant
            </p>
          </div>
        </header>
        <div className="flex items-center justify-between gap-4">
          <span className="text-sm tabular-nums">
            {cacheBytes === null
              ? "—"
              : cacheBytes === 0
                ? "Empty"
                : formatBytes(cacheBytes)}
          </span>
          <Button
            size="sm"
            variant="secondary"
            disabled={!tauri || clearing || cacheBytes === 0}
            onClick={() => void clearCache()}
          >
            <Broom data-icon="inline-start" />
            {clearing ? "Clearing…" : "Clear"}
          </Button>
        </div>
        <p className="text-xs text-muted-foreground">
          Only these cached copies are removed. Your PNGs, OCR text and search
          results are untouched — thumbnails rebuild from the originals as you
          scroll.
        </p>
        {!tauri ? (
          <Badge variant="secondary">Needs the desktop shell</Badge>
        ) : null}
      </section>

      <section className="space-y-4 rounded-2xl border bg-card p-5 shadow-sm">
        <header className="flex items-center gap-2.5">
          <FolderPlus size={18} className="text-muted-foreground" />
          <div>
            <h2 className="font-medium">Watched folders</h2>
            <p className="text-xs text-muted-foreground">
              Extra places to look for images, besides where captures are saved
            </p>
          </div>
        </header>

        <ul className="space-y-2">
          {scanFolders.length === 0 ? (
            <li className="text-sm text-muted-foreground">
              None yet — only {settings.saveDir.trim() || "Pictures"} is
              watched.
            </li>
          ) : (
            scanFolders.map((dir) => (
              <li
                key={dir}
                className="flex items-center gap-2 rounded-xl border bg-background px-3 py-2"
              >
                <FolderOpen
                  size={16}
                  className="shrink-0 text-muted-foreground"
                />
                <span className="min-w-0 flex-1 truncate text-sm" title={dir}>
                  {dir}
                </span>
                <Button
                  size="icon"
                  variant="ghost"
                  className="size-7 shrink-0"
                  title="Stop watching"
                  disabled={!tauri}
                  onClick={() => removeScanFolder(dir)}
                >
                  <Trash />
                </Button>
              </li>
            ))
          )}
        </ul>

        <div className="flex gap-2">
          <Input
            value={newFolder}
            onChange={(e) => setNewFolder(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") addScanFolder()
            }}
            placeholder="/home/you/Downloads"
            disabled={!tauri}
            className="ui-selectable"
          />
          <Button
            variant="secondary"
            disabled={!tauri || !newFolder.trim()}
            onClick={addScanFolder}
          >
            <FolderPlus data-icon="inline-start" />
            Add
          </Button>
        </div>
        <p className="text-xs text-muted-foreground">
          Only png, jpg, jpeg, webp and gif files are indexed. Subfolders are
          not scanned. The folder must already exist — this app never creates
          directories you did not ask for.
        </p>
        {!tauri ? (
          <Badge variant="secondary">Needs the desktop shell</Badge>
        ) : null}
      </section>
    </div>
  )
}
