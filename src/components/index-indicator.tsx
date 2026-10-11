import { useCallback, useEffect, useState } from "react"
import { isTauri } from "@tauri-apps/api/core"
import { listen } from "@tauri-apps/api/event"
import { Circle } from "@phosphor-icons/react"

import { indexPause, indexStatus, type IndexStatus } from "@/lib/screenshot"

const POLL_MS = 4000

type State =
  | { kind: "done" }
  | { kind: "loading" }
  | { kind: "working"; embedded: number; total: number }
  | { kind: "paused"; embedded: number; total: number }
  | { kind: "ocr" }

/**
 * Compact indexing indicator.
 *
 * Deliberately a spinner and not a progress bar: one screenshot indexes in a
 * couple of seconds and a full backfill ran at roughly 1.4s per image, so a bar
 * would sit near-empty or twitch constantly. The useful signal is *which state*
 * it is in -- "waiting for the model" and "stuck" look identical without saying
 * so, and the model takes about two minutes to load.
 */
function describe(s: IndexStatus): State {
  const outstanding = s.embedded < s.total || s.ocred < s.total
  if (s.paused) {
    return outstanding
      ? { kind: "paused", embedded: s.embedded, total: s.total }
      : { kind: "done" }
  }
  if (!outstanding) return { kind: "done" }
  if (!s.server_ready) return { kind: "loading" }
  if (s.ocred < s.total) return { kind: "ocr" }
  return { kind: "working", embedded: s.embedded, total: s.total }
}

export function IndexIndicator() {
  const tauri = isTauri()
  const [status, setStatus] = useState<IndexStatus | null>(null)

  const refresh = useCallback(() => {
    if (!tauri) return
    indexStatus()
      .then(setStatus)
      .catch(() => setStatus(null))
  }, [tauri])

  useEffect(() => {
    if (!tauri) return
    void refresh()
    const poll = setInterval(() => void refresh(), POLL_MS)
    // The indexer also pushes after every batch, so a backfill shows progress
    // immediately instead of lagging up to one poll interval behind.
    const un = listen<IndexStatus>("index-progress", (e) =>
      setStatus(e.payload),
    )
    return () => {
      clearInterval(poll)
      void un.then((f) => f())
    }
  }, [tauri, refresh])

  if (!status) return null
  const state = describe(status)
  if (state.kind === "done") return null
  const paused = state.kind === "paused"

  return (
    <button
      type="button"
      title={paused ? "Resume indexing" : "Pause indexing"}
      onClick={() => {
        void indexPause(!paused)
        void refresh()
      }}
      className="flex items-center gap-2 rounded-full border bg-card px-3 py-1.5 text-xs transition-colors hover:bg-muted"
      role="status"
      aria-live="polite"
    >
      <Circle
        size={12}
        weight="fill"
        className={
          paused ? "shrink-0 text-muted-foreground" : "shrink-0 animate-pulse text-primary"
        }
      />
      <span className="text-muted-foreground">
        {state.kind === "loading" && "Loading model…"}
        {state.kind === "ocr" && "Reading text…"}
        {state.kind === "working" &&
          `Indexing ${state.embedded} of ${state.total}`}
        {state.kind === "paused" &&
          `Paused at ${state.embedded} of ${state.total}`}
      </span>
    </button>
  )
}