import { useCallback, useEffect, useState } from "react"
import { emit } from "@tauri-apps/api/event"

import { trackedInvoke } from "@/lib/tauri"

export interface RegionRect {
  x: number
  y: number
  width: number
  height: number
}

const MIN_SIZE = 4

export const REGION_SELECTED = "xreenshoty:region-selected"
export const REGION_CANCELLED = "xreenshoty:region-cancelled"

export function RegionOverlay() {
  const [backdrop, setBackdrop] = useState<string | null>(null)
  const [anchor, setAnchor] = useState<{ x: number; y: number } | null>(null)
  const [cursor, setCursor] = useState<{ x: number; y: number } | null>(null)

  useEffect(() => {
    let cancelled = false
    trackedInvoke<string>("backdrop_data_url")
      .then((url) => {
        if (!cancelled) setBackdrop(url)
      })
      .catch(() => undefined)
    return () => {
      cancelled = true
    }
  }, [])

  // The overlay never closes itself; the main window tears it down after saving.
  const finish = useCallback(async (rect: RegionRect | null) => {
    await emit(rect ? REGION_SELECTED : REGION_CANCELLED, rect)
  }, [])

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault()
        void finish(null)
      }
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [finish])

  function handlePointerDown(event: React.PointerEvent) {
    if (event.button !== 0) return
    event.preventDefault()
    setAnchor({ x: event.clientX, y: event.clientY })
    setCursor({ x: event.clientX, y: event.clientY })
  }

  function handlePointerUp(event: React.PointerEvent) {
    const start = anchor
    setAnchor(null)
    if (!start) return

    const x = Math.round(Math.min(start.x, event.clientX))
    const y = Math.round(Math.min(start.y, event.clientY))
    const width = Math.round(Math.abs(event.clientX - start.x))
    const height = Math.round(Math.abs(event.clientY - start.y))

    void finish(
      width < MIN_SIZE || height < MIN_SIZE
        ? null
        : { x, y, width, height },
    )
  }

  const rect = useRect(anchor, cursor)

  // Layout-critical geometry is inline rather than Tailwind: this page is a
  // separate webview, and if its stylesheet ever fails to load we still want
  // the backdrop and the selection box visible.
  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        cursor: "crosshair",
        overflow: "hidden",
        background: "#0a0a0a",
        userSelect: "none",
      }}
      onPointerDown={handlePointerDown}
      onPointerMove={(event) =>
        setCursor({ x: event.clientX, y: event.clientY })
      }
      onPointerUp={handlePointerUp}
      onContextMenu={(event) => {
        event.preventDefault()
        void finish(null)
      }}
    >
      {backdrop ? (
        <img
          src={backdrop}
          alt=""
          draggable={false}
          style={{
            position: "absolute",
            inset: 0,
            width: "100%",
            height: "100%",
            objectFit: "fill",
          }}
        />
      ) : null}

      {rect ? (
        <div
          style={{
            position: "absolute",
            left: rect.x,
            top: rect.y,
            width: rect.width,
            height: rect.height,
            pointerEvents: "none",
            border: "2px solid #ffffff",
            boxShadow: "0 0 0 9999px rgba(0, 0, 0, 0.25)",
          }}
        >
          <span
            style={{
              position: "absolute",
              top: -26,
              left: 0,
              padding: "2px 6px",
              borderRadius: 4,
              background: "#ffffff",
              color: "#0a0a0a",
              font: "600 12px/1.2 monospace",
            }}
          >
            {Math.round(rect.width)} × {Math.round(rect.height)}
          </span>
        </div>
      ) : null}

      <div
        style={{
          position: "absolute",
          left: 0,
          right: 0,
          bottom: 32,
          display: "flex",
          justifyContent: "center",
          pointerEvents: "none",
        }}
      >
        <span
          style={{
            padding: "6px 16px",
            borderRadius: 999,
            background: "rgba(0, 0, 0, 0.7)",
            color: "rgba(255, 255, 255, 0.9)",
            font: "12px/1.4 sans-serif",
            opacity: anchor ? 1 : 0.75,
          }}
        >
          {anchor
            ? "Release to capture · Escape or right-click to cancel"
            : "Drag to select a region"}
        </span>
      </div>
    </div>
  )
}

function useRect(
  anchor: { x: number; y: number } | null,
  cursor: { x: number; y: number } | null,
) {
  if (!anchor || !cursor) return null
  return {
    x: Math.min(anchor.x, cursor.x),
    y: Math.min(anchor.y, cursor.y),
    width: Math.abs(cursor.x - anchor.x),
    height: Math.abs(cursor.y - anchor.y),
  }
}
