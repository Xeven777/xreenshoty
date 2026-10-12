import { useCallback, useEffect, useRef, useState } from "react"
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
  const rectRef = useRef<HTMLDivElement | null>(null)
  const labelRef = useRef<HTMLSpanElement | null>(null)
  // The dim is four solid strips rather than one 9999px box-shadow: the shadow
  // re-painted the entire monitor on every drag frame, because the hole in it
  // changed size each time. Strips are plain fills and stay cheap.
  const topStripRef = useRef<HTMLDivElement | null>(null)
  const bottomStripRef = useRef<HTMLDivElement | null>(null)
  const leftStripRef = useRef<HTMLDivElement | null>(null)
  const rightStripRef = useRef<HTMLDivElement | null>(null)
  const frameRef = useRef(0)

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
    // Capture keeps the drag alive when the button is released past the edge of
    // the monitor. Without it `pointerup` never arrives, so no result event is
    // emitted and the main window waits forever (H1).
    event.currentTarget.setPointerCapture(event.pointerId)
    setAnchor({ x: event.clientX, y: event.clientY })
  }

  // Direct-DOM write, rAF-coalesced: setState per pointermove re-rendered the
  // whole overlay (including the fullscreen backdrop img) at drag rate.
  function scheduleRect(start: { x: number; y: number }, x: number, y: number) {
    if (frameRef.current) return
    frameRef.current = requestAnimationFrame(() => {
      frameRef.current = 0
      const el = rectRef.current
      if (!el) return
      const left = Math.min(start.x, x)
      const top = Math.min(start.y, y)
      const width = Math.abs(x - start.x)
      const height = Math.abs(y - start.y)
      el.style.left = `${left}px`
      el.style.top = `${top}px`
      el.style.width = `${width}px`
      el.style.height = `${height}px`
      if (labelRef.current) {
        labelRef.current.textContent = `${Math.round(width)} × ${Math.round(height)}`
      }
      const topStrip = topStripRef.current
      if (topStrip) topStrip.style.height = `${top}px`
      const bottomStrip = bottomStripRef.current
      if (bottomStrip) bottomStrip.style.top = `${top + height}px`
      const leftStrip = leftStripRef.current
      if (leftStrip) {
        leftStrip.style.top = `${top}px`
        leftStrip.style.height = `${height}px`
        leftStrip.style.width = `${left}px`
      }
      const rightStrip = rightStripRef.current
      if (rightStrip) {
        rightStrip.style.left = `${left + width}px`
        rightStrip.style.top = `${top}px`
        rightStrip.style.height = `${height}px`
      }
    })
  }

  function handlePointerMove(event: React.PointerEvent) {
    const start = anchor
    if (!start) return
    scheduleRect(start, event.clientX, event.clientY)
  }

  function handlePointerUp(event: React.PointerEvent) {
    const start = anchor
    setAnchor(null)
    if (frameRef.current) {
      cancelAnimationFrame(frameRef.current)
      frameRef.current = 0
    }
    if (!start) return

    const x = Math.round(Math.min(start.x, event.clientX))
    const y = Math.round(Math.min(start.y, event.clientY))
    const width = Math.round(Math.abs(event.clientX - start.x))
    const height = Math.round(Math.abs(event.clientY - start.y))

    void finish(
      width < MIN_SIZE || height < MIN_SIZE ? null : { x, y, width, height }
    )
  }

  // Mounted once per drag (anchor down/up is the only state change); the live
  // geometry is written straight to the element in scheduleRect.
  const rect = anchor

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
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
      onPointerCancel={(event) => {
        // The browser took the gesture over (touch pan, compositor grab).
        // Abandoning still has to emit, or the main window stays wedged.
        event.preventDefault()
        void finish(null)
      }}
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
        <>
          <div
            style={{
              position: "absolute",
              inset: 0,
              pointerEvents: "none",
            }}
          >
            <div
              ref={topStripRef}
              style={{
                position: "absolute",
                left: 0,
                top: 0,
                right: 0,
                height: rect.y,
                background: "rgba(0, 0, 0, 0.25)",
              }}
            />
            <div
              ref={bottomStripRef}
              style={{
                position: "absolute",
                left: 0,
                right: 0,
                top: rect.y,
                bottom: 0,
                background: "rgba(0, 0, 0, 0.25)",
              }}
            />
            <div
              ref={leftStripRef}
              style={{
                position: "absolute",
                left: 0,
                top: rect.y,
                width: 0,
                height: 0,
                background: "rgba(0, 0, 0, 0.25)",
              }}
            />
            <div
              ref={rightStripRef}
              style={{
                position: "absolute",
                left: rect.x,
                right: 0,
                top: rect.y,
                height: 0,
                background: "rgba(0, 0, 0, 0.25)",
              }}
            />
          </div>
          <div
            ref={rectRef}
            style={{
              position: "absolute",
              left: rect.x,
              top: rect.y,
              width: 0,
              height: 0,
              pointerEvents: "none",
              border: "2px solid #ffffff",
            }}
          >
            <span
              ref={labelRef}
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
              0 × 0
            </span>
          </div>
        </>
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
