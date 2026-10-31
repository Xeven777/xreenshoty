import { useCallback, useEffect, useState } from "react"

export interface Settings {
  saveDir: string
  /** Extra folders to watch for images, on top of `saveDir`. */
  scanFolders: string[]
  delaySecs: number
  shortcutFullscreen: string
  shortcutWindow: string
  notifyOnSave: boolean
  /** Opt-in face indexing (Milestone 7). Off by default; biometric-class. */
  faceIndexing: boolean
}

const KEY = "xreenshoty.settings"

export const DEFAULT_SETTINGS: Settings = {
  saveDir: "",
  scanFolders: [],
  delaySecs: 0,
  shortcutFullscreen: "Control+Shift+S",
  shortcutWindow: "Control+Shift+W",
  notifyOnSave: true,
  faceIndexing: false,
}

export const MAX_DELAY = 30

function clampDelay(value: number) {
  if (!Number.isFinite(value)) return 0
  return Math.min(MAX_DELAY, Math.max(0, Math.round(value)))
}

/** Stored settings are user-editable and may predate a field, so coerce rather
 *  than trust. A `scanFolders` that is not an array of strings would otherwise
 *  reach `.map` and break the whole Settings tab, a string `delaySecs` would
 *  make the countdown arithmetic produce `NaN`, and a non-boolean
 *  `notifyOnSave` would be read as truthy. Every field is validated, not just
 *  the obvious one (M8). */
function sanitise(stored: Partial<Settings>): Settings {
  const raw = stored as Record<string, unknown>
  const str = (v: unknown, fallback: string) =>
    typeof v === "string" ? v : fallback
  return {
    saveDir: str(raw.saveDir, DEFAULT_SETTINGS.saveDir),
    scanFolders: Array.isArray(raw.scanFolders)
      ? raw.scanFolders.filter((d): d is string => typeof d === "string")
      : DEFAULT_SETTINGS.scanFolders,
    delaySecs:
      typeof raw.delaySecs === "number"
        ? clampDelay(raw.delaySecs)
        : DEFAULT_SETTINGS.delaySecs,
    shortcutFullscreen: str(
      raw.shortcutFullscreen,
      DEFAULT_SETTINGS.shortcutFullscreen
    ),
    shortcutWindow: str(raw.shortcutWindow, DEFAULT_SETTINGS.shortcutWindow),
    notifyOnSave:
      typeof raw.notifyOnSave === "boolean"
        ? raw.notifyOnSave
        : DEFAULT_SETTINGS.notifyOnSave,
    faceIndexing:
      typeof raw.faceIndexing === "boolean"
        ? raw.faceIndexing
        : DEFAULT_SETTINGS.faceIndexing,
  }
}

export function useSettings() {
  const [settings, setSettings] = useState<Settings>(() => {
    try {
      const raw = localStorage.getItem(KEY)
      if (!raw) return DEFAULT_SETTINGS
      return sanitise(JSON.parse(raw) as Partial<Settings>)
    } catch {
      return DEFAULT_SETTINGS
    }
  })

  useEffect(() => {
    try {
      localStorage.setItem(KEY, JSON.stringify(settings))
    } catch {
      /* private mode — keep in memory only */
    }
  }, [settings])

  const update = useCallback(
    <K extends keyof Settings>(key: K, value: Settings[K]) => {
      setSettings((prev) => {
        const next = { ...prev, [key]: value }
        if (key === "delaySecs") next.delaySecs = clampDelay(value as number)
        return next
      })
    },
    []
  )

  return { settings, update }
}
