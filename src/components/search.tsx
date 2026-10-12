import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { isTauri } from "@tauri-apps/api/core"
import {
  Eye,
  ImageIcon,
  MagnifyingGlass,
  Microphone,
  MicrophoneSlash,
  SpinnerGap,
  Stack,
  TextAa,
  XIcon,
} from "@phosphor-icons/react"
import { AnimatePresence, motion, useInView } from "motion/react"

import { Badge } from "@/components/ui/badge"
import { ShotCard, ShotPreview, usePreview } from "@/components/shot"
import { Input } from "@/components/ui/input"
import { errorText, toastFailed } from "@/lib/toast"
import { dirOf } from "@/lib/utils"
import {
  openFolderSafe,
  searchHybrid,
  searchSimilar,
  searchText,
  voiceCancel,
  voiceStart,
  voiceStop,
  type SearchHit,
  type Screenshot,
} from "@/lib/screenshot"
import { useThumbnails } from "@/lib/use-thumbnails"

const DEBOUNCE_MS = 350
const LIMIT = 60

const MODES = [
  {
    id: "combined",
    label: "Combined",
    icon: Stack,
    blurb:
      "Matches the text a screenshot contains and what it looks like, ranked by agreement between the two.",
    placeholder: "e.g. permission denied, a terminal full of red errors",
    empty: "Try “login screen”, “bookmarks”, “red error dialog”…",
  },
  {
    id: "ocr",
    label: "OCR",
    icon: TextAa,
    blurb:
      "Matches the words a screenshot contains. Best for exact names, error codes and file paths.",
    placeholder: "e.g. permission denied, NFTS, ECONNREFUSED",
    empty: "Only screenshots with readable text can match here.",
  },
  {
    id: "visual",
    label: "Visual",
    icon: Eye,
    blurb:
      "Describes what a screenshot looks like. Finds images with no readable text at all.",
    placeholder: "e.g. a terminal full of red errors",
    empty: "Try “login screen”, “chart going up”, “red error dialog”…",
  },
] as const

type Mode = (typeof MODES)[number]["id"]

function VoiceButton({ onText }: { onText: (t: string) => void }) {
  const [state, setState] = useState<"idle" | "recording" | "working">("idle")
  const [error, setError] = useState<string | null>(null)
  const stateRef = useRef(state)
  const buttonRef = useRef<HTMLButtonElement>(null)
  // The Search tab is keepMounted, so a hidden button still exists — without
  // this gate the recording pulse and ping ring kept running off-screen.
  const inView = useInView(buttonRef)
  useEffect(() => {
    stateRef.current = state
  }, [state])

  async function toggle() {
    if (stateRef.current === "working") return
    if (stateRef.current === "recording") {
      setState("working")
      try {
        const text = (await voiceStop()).trim()
        if (text) onText(text)
        else setError("Heard nothing — try again closer to the mic.")
      } catch (err) {
        setError(errorText(err))
      }
      setState("idle")
      return
    }
    setError(null)
    try {
      await voiceStart()
      setState("recording")
    } catch (err) {
      setError(errorText(err))
    }
  }

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape" && stateRef.current === "recording") {
        void voiceCancel().catch(() => undefined)
        setState("idle")
      }
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [])

  const recording = state === "recording"
  const pulse = recording && inView

  return (
    <>
      <motion.button
        ref={buttonRef}
        type="button"
        title={
          recording
            ? "Tap to stop and search"
            : "Tap to talk, tap again to search"
        }
        aria-label="Voice search"
        aria-pressed={recording}
        onClick={() => void toggle()}
        whileTap={{ scale: 0.94 }}
        animate={pulse ? { scale: [1, 1.12, 1] } : { scale: 1 }}
        transition={
          pulse
            ? { duration: 1.4, repeat: Infinity, ease: "easeInOut" }
            : { type: "spring", stiffness: 500, damping: 22 }
        }
        className={`relative grid size-9 place-items-center rounded-full transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring/40 ${
          recording
            ? "text-destructive-foreground bg-destructive shadow-lg"
            : "bg-brand text-primary-foreground shadow-brand hover:opacity-90"
        }`}
      >
        {recording ? (
          <span
            className={`absolute inset-0 rounded-full bg-destructive/40 ${
              pulse ? "animate-ping" : ""
            }`}
          />
        ) : null}
        {state === "working" ? (
          <SpinnerGap size={17} className="animate-spin" />
        ) : recording ? (
          <MicrophoneSlash size={17} weight="fill" />
        ) : (
          <Microphone size={17} weight="fill" />
        )}
      </motion.button>
      <AnimatePresence>
        {error ? (
          <motion.span
            initial={{ opacity: 0, y: -4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -4 }}
            transition={{ duration: 0.18 }}
            className="absolute top-full right-0 z-10 mt-1 max-w-55 rounded-lg border bg-card p-2 text-xs text-destructive shadow-lg"
          >
            {error}
          </motion.span>
        ) : null}
      </AnimatePresence>
    </>
  )
}

function runSearch(mode: Mode, query: string): Promise<SearchHit[]> {
  if (mode === "ocr") return searchText(query, LIMIT)
  if (mode === "visual") return searchSimilar(query, LIMIT)
  return searchHybrid(query, LIMIT)
}

export function Search() {
  const tauri = isTauri()
  const preview = usePreview()
  const [query, setQuery] = useState("")
  const [mode, setMode] = useState<Mode>("combined")
  const [settled, setSettled] = useState("")
  const [results, setResults] = useState<SearchHit[]>([])
  const [elapsed, setElapsed] = useState(0)
  const [failure, setFailure] = useState<{ key: string; msg: string } | null>(
    null
  )
  const seq = useRef(0)

  const term = query.trim()
  const key = `${mode}:${term}`
  const active = MODES.find((m) => m.id === mode)!
  const searching = tauri && term.length > 0 && key !== settled
  const searched = tauri && term.length > 0 && key === settled
  const failed = searched && failure?.key === key
  const shown = useMemo(
    () => (searched && !failed ? results : []),
    [searched, failed, results]
  )
  const scoreByPath = useMemo(
    () => new Map(results.map((r) => [r.path, r])),
    [results]
  )

  const thumbs = useThumbnails(useMemo(() => shown.map((r) => r.path), [shown]))

  const openFolderOf = useCallback((item: Screenshot) => {
    void openFolderSafe(dirOf(item.path))
  }, [])

  useEffect(() => {
    if (!tauri || term.length === 0) return

    const n = ++seq.current
    const k = key
    const t = term
    const m = mode
    const timer = setTimeout(async () => {
      const started = performance.now()
      try {
        const found = await runSearch(m, t)
        if (seq.current !== n) return
        setResults(found)
        setFailure(null)
        setElapsed(Math.max(1, Math.round(performance.now() - started)))
      } catch (e) {
        if (seq.current !== n) return
        setFailure({ key: k, msg: errorText(e) })
        toastFailed("Search failed", e, () => void searchNow())
      } finally {
        if (seq.current === n) setSettled(k)
      }
    }, DEBOUNCE_MS)

    return () => {
      clearTimeout(timer)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [term, mode, key, tauri])

  async function searchNow() {
    if (!tauri || term.length === 0) return
    const n = ++seq.current
    const k = key
    const t = term
    const started = performance.now()
    try {
      const found = await runSearch(mode, t)
      if (seq.current !== n) return
      setResults(found)
      setFailure(null)
      setElapsed(Math.max(1, Math.round(performance.now() - started)))
    } catch (e) {
      if (seq.current !== n) return
      setFailure({ key: k, msg: errorText(e) })
      toastFailed("Search failed", e, () => void searchNow())
    } finally {
      if (seq.current === n) setSettled(k)
    }
  }

  return (
    <div className="space-y-4">
      <section className="space-y-4 rounded-2xl border bg-card p-5 shadow-sm">
        <header className="flex flex-wrap items-end justify-between gap-3">
          <div className="min-w-0">
            <h2 className="font-medium">Search screenshots</h2>
            <p className="text-xs text-muted-foreground">{active.blurb}</p>
          </div>
          <div
            role="group"
            aria-label="Search mode"
            className="flex rounded-full bg-muted p-1"
          >
            {MODES.map((opt) => (
              <button
                key={opt.id}
                type="button"
                aria-pressed={mode === opt.id}
                title={opt.blurb}
                onClick={() => setMode(opt.id)}
                className={`flex items-center gap-1.5 rounded-full px-3 py-1 text-xs font-medium transition-[color,background-color,box-shadow] duration-150 outline-none focus-visible:ring-2 focus-visible:ring-ring/40 ${
                  mode === opt.id
                    ? "bg-card text-foreground shadow-sm"
                    : "text-muted-foreground hover:text-foreground"
                }`}
              >
                <opt.icon size={14} data-icon="inline-start" />
                {opt.label}
              </button>
            ))}
          </div>
        </header>

        <div className="relative">
          <MagnifyingGlass
            size={16}
            className="pointer-events-none absolute top-1/2 left-3.5 -translate-y-1/2 text-muted-foreground"
          />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault()
                void searchNow()
              } else if (e.key === "Escape" && query) {
                setQuery("")
              }
            }}
            placeholder={active.placeholder}
            disabled={!tauri}
            aria-label="Search screenshots"
            className="h-11 pr-24 pl-10 text-[15px] shadow-sm"
          />
          <span className="absolute top-1/2 right-3 flex -translate-y-1/2 items-center gap-1">
            <VoiceButton onText={(t) => setQuery(t)} />
            {searching ? (
              <SpinnerGap
                size={16}
                className="animate-spin text-muted-foreground"
              />
            ) : query ? (
              <button
                type="button"
                title="Clear (Esc)"
                aria-label="Clear search"
                onClick={() => setQuery("")}
                className="grid size-6 place-items-center rounded-full text-muted-foreground transition-colors outline-none hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/40"
              >
                <XIcon size={14} />
              </button>
            ) : (
              <kbd className="hidden rounded-md border bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground sm:block">
                ⏎
              </kbd>
            )}
          </span>
        </div>

        {failed ? (
          <p className="rounded-xl bg-destructive/10 p-3 text-sm text-destructive">
            {failure.msg}
          </p>
        ) : null}

        {searched && !failed ? (
          <div className="flex items-center gap-2">
            <Badge variant="secondary">{shown.length}</Badge>
            <span className="text-xs text-muted-foreground">
              {shown.length === 1 ? "screenshot" : "screenshots"} matching
              {mode === "combined" ? "text + appearance · " : ""}
              {elapsed} ms
            </span>
          </div>
        ) : null}
      </section>

      {searching && shown.length === 0 ? (
        <div
          aria-label="Searching"
          className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4"
        >
          {Array.from({ length: 8 }).map((_, i) => (
            <div key={i} className="overflow-hidden rounded-xl border bg-card">
              <div className="aspect-video animate-pulse bg-muted" />
              <div className="space-y-1.5 p-2.5">
                <div className="h-3 w-2/3 animate-pulse rounded bg-muted" />
                <div className="h-2.5 w-1/2 animate-pulse rounded bg-muted" />
              </div>
            </div>
          ))}
        </div>
      ) : shown.length === 0 ? (
        <div className="flex items-center gap-3 rounded-xl border bg-card p-6 text-sm text-muted-foreground">
          <ImageIcon size={20} className="shrink-0" />
          <div>
            <p className="font-medium text-foreground">
              {failed
                ? "Search failed"
                : searched
                  ? "Nothing matched"
                  : "Search words or describe a screenshot"}
            </p>
            <p className="text-xs">
              {failed
                ? "See the message above."
                : searched
                  ? "Try fewer or more common words."
                  : "Try “login screen”, “terminal”, “red error dialog”…"}
            </p>
          </div>
        </div>
      ) : (
        <div className="shot-grid grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
          {/* AnimatePresence renders no DOM of its own, so the cards stay
              direct grid children while its direct children (the cards) are
              what gets exit-animated. popLayout pops the leaving card out of
              flow instead of leaving a gap. */}
          <AnimatePresence mode="popLayout" initial={false}>
            {shown.map((item, i) => (
              <ShotCard
                key={item.path}
                item={item}
                index={i}
                thumb={thumbs[item.path]}
                snippet={mode === "visual" ? null : item.snippet}
                score={mode === "ocr" ? null : item.score}
                scoreZ={mode === "ocr" ? null : item.score_z}
                onOpen={preview.show}
                onOpenFolder={openFolderOf}
              />
            ))}
          </AnimatePresence>
        </div>
      )}

      <ShotPreview
        item={preview.item}
        score={
          preview.item
            ? (scoreByPath.get(preview.item.path)?.score ?? null)
            : null
        }
        scoreZ={
          preview.item
            ? (scoreByPath.get(preview.item.path)?.score_z ?? null)
            : null
        }
        onClose={preview.hide}
        onOpenFolder={openFolderOf}
      />
    </div>
  )
}
