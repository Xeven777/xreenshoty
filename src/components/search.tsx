import { useEffect, useMemo, useState } from "react"
import { isTauri } from "@tauri-apps/api/core"
import {
  Eye,
  Image as ImageIcon,
  MagnifyingGlass,
  SpinnerGap,
  TextAa,
} from "@phosphor-icons/react"

import { Badge } from "@/components/ui/badge"
import { ShotCard, ShotPreview, usePreview } from "@/components/shot"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  openFolder,
  searchSimilar,
  searchText,
  type Screenshot,
} from "@/lib/screenshot"
import { useThumbnails } from "@/lib/use-thumbnails"

const DEBOUNCE_MS = 250
type Mode = "text" | "visual"

export function Search({ saveDir }: { saveDir: string | null }) {
  const tauri = useMemo(() => isTauri(), [])
  const preview = usePreview()
  const [query, setQuery] = useState("")
  const [mode, setMode] = useState<Mode>("text")
  const [visualError, setVisualError] = useState<string | null>(null)
  const [settled, setSettled] = useState("")
  const [results, setResults] = useState<Screenshot[]>([])
  const [error, setError] = useState<string | null>(null)

  const term = query.trim()
  // Derived rather than stored: a new keystroke must not leave the previous
  // term's results on screen, and "searching" covers both the debounce wait
  // and the in-flight request.
  const searching = tauri && term.length > 0 && term !== settled
  const searched = tauri && term.length > 0 && term === settled
  const shown = useMemo(() => (searched ? results : []), [searched, results])

  const thumbs = useThumbnails(useMemo(() => shown.map((r) => r.path), [shown]))

  useEffect(() => {
    if (!tauri || term.length === 0) return

    let cancelled = false
    const timer = setTimeout(async () => {
      try {
        let found: Screenshot[]
        if (mode === "visual") {
          found = await searchSimilar(term, 60)
          if (cancelled) return
          setVisualError(null)
        } else {
          found = await searchText(term, 60)
          if (cancelled) return
        }
        setResults(found)
        setError(null)
      } catch (e) {
        if (cancelled) return
        const msg = String(e)
        if (mode === "visual") setVisualError(msg)
        else setError(msg)
      } finally {
        if (!cancelled) setSettled(term)
      }
    }, DEBOUNCE_MS)

    return () => {
      cancelled = true
      clearTimeout(timer)
    }
  }, [term, mode, tauri])

  return (
    <div className="space-y-4">
      <section className="space-y-4 rounded-2xl border bg-card p-5 shadow-sm">
        <header>
          <h2 className="font-medium">Search screenshots</h2>
          <p className="text-xs text-muted-foreground">
            Matches any text a screenshot contains. Multiple words widen the
            search.
          </p>
        </header>

        <div className="flex gap-2">
          <Button
            size="xs"
            variant={mode === "text" ? "default" : "ghost"}
            onClick={() => setMode("text")}
          >
            <TextAa data-icon="inline-start" />
            Words
          </Button>
          <Button
            size="xs"
            variant={mode === "visual" ? "default" : "ghost"}
            onClick={() => setMode("visual")}
          >
            <Eye data-icon="inline-start" />
            What&apos;s in the picture
          </Button>
          {mode === "visual" ? (
            <span className="self-center text-xs text-muted-foreground">
              needs the embedding model
            </span>
          ) : null}
        </div>

        <div className="relative">
          <MagnifyingGlass
            size={16}
            className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-muted-foreground"
          />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="e.g. permission denied, bookmarks, error"
            disabled={!tauri}
            className="pl-9 pr-9"
          />
          {searching ? (
            <SpinnerGap
              size={16}
              className="animate-spin absolute top-1/2 right-3 -translate-y-1/2 text-muted-foreground"
            />
          ) : null}
        </div>

        {visualError ? (
          <p className="rounded-xl bg-destructive/10 p-3 text-sm text-destructive">
            Visual search is unavailable: {visualError}
          </p>
        ) : null}

        {error ? (
          <p className="rounded-xl bg-destructive/10 p-3 text-sm text-destructive">
            {error}
          </p>
        ) : null}

        {searched ? (
          <div className="flex items-center gap-2">
            <Badge variant="secondary">{shown.length}</Badge>
            <span className="text-xs text-muted-foreground">
              {shown.length === 1 ? "screenshot" : "screenshots"} matching
            </span>
          </div>
        ) : null}
      </section>

      {shown.length === 0 ? (
        <div className="flex items-center gap-3 rounded-2xl border bg-card p-6 text-sm text-muted-foreground">
          <ImageIcon size={20} />
          {searched
            ? "Nothing matched. Try fewer or more common words."
            : "Type a word to search the text inside your screenshots."}
        </div>
      ) : (
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
          {shown.map((item) => (
            <ShotCard
              key={item.path}
              item={item}
              thumb={thumbs[item.path]}
              onOpen={preview.show}
              onOpenFolder={() => openFolder(saveDir).catch(() => undefined)}
            />
          ))}
        </div>
      )}

      <ShotPreview
        item={preview.item}
        onClose={preview.hide}
        onOpenFolder={() => openFolder(saveDir).catch(() => undefined)}
      />
    </div>
  )
}