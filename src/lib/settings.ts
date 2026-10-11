import { useCallback, useEffect, useState } from "react"

export interface Settings {
  saveDir: string
  delaySecs: number
  shortcutFullscreen: string
  shortcutWindow: string
  notifyOnSave: boolean
}

const KEY = "xreenshoty.settings"

export const DEFAULT_SETTINGS: Settings = {
  saveDir: "",
  delaySecs: 0,
  shortcutFullscreen: "Control+Shift+S",
  shortcutWindow: "Control+Shift+W",
  notifyOnSave: true,
}

export const MAX_DELAY = 30

function clampDelay(value: number) {
  if (!Number.isFinite(value)) return 0
  return Math.min(MAX_DELAY, Math.max(0, Math.round(value)))
}

export function useSettings() {
  const [settings, setSettings] = useState<Settings>(() => {
    try {
      const raw = localStorage.getItem(KEY)
      if (!raw) return DEFAULT_SETTINGS
      return { ...DEFAULT_SETTINGS, ...(JSON.parse(raw) as Partial<Settings>) }
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
