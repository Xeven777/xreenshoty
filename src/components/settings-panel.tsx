import { useEffect, useMemo, useState } from "react"
import { isTauri } from "@tauri-apps/api/core"
import {
  Bell,
  Check,
  FolderOpen,
  Keyboard,
  Minus,
  Plus,
  Warning,
  X,
} from "@phosphor-icons/react"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { MAX_DELAY, type Settings } from "@/lib/settings"
import { getDefaultSaveDir, openFolder } from "@/lib/screenshot"
import { eventToAccelerator, prettyAccelerator } from "@/lib/shortcuts"

export function SettingsPanel({
  settings,
  update,
  shortcutError,
}: {
  settings: Settings
  update: <K extends keyof Settings>(key: K, value: Settings[K]) => void
  shortcutError: string | null
}) {
  const tauri = useMemo(() => isTauri(), [])
  const [recording, setRecording] = useState<
    "shortcutFullscreen" | "shortcutWindow" | null
  >(null)

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
                openFolder(settings.saveDir.trim() || null).catch(
                  () => undefined
                )
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
    </div>
  )
}
