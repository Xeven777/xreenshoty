import { useEffect, useState } from "react"
import {
  ArrowsOut,
  CaretDown,
  Check,
  Copy,
  FolderOpen,
  Trash,
  X,
} from "@phosphor-icons/react"

import { Button } from "@/components/ui/button"
import {
  getThumbnail,
  openFile,
  screenshotDetail,
  type Screenshot,
  type ScreenshotDetail,
} from "@/lib/screenshot"

export function fileName(path: string) {
  return path.split(/[/\\]/).pop() ?? path
}

export function dirOf(path: string) {
  const i = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"))
  return i > 0 ? path.slice(0, i) : path
}

const PREVIEW_SIZE = 1600
const START = "\u001f"
const END = "\u001e"

export function usePreview() {
  const [item, setItem] = useState<Screenshot | null>(null)
  return { item, show: setItem, hide: () => setItem(null) }
}

export function Snippet({ text }: { text: string }) {
  const parts = text.split(new RegExp(`[${START}${END}]`))
  return (
    <>
      {parts.map((part, i) =>
        i % 2 === 1 ? (
          <mark
            key={i}
            className="rounded bg-primary/20 px-0.5 text-foreground"
          >
            {part}
          </mark>
        ) : (
          <span key={i}>{part}</span>
        )
      )}
    </>
  )
}

/**
 * Similarity readout, shown as a percentage.
 *
 * Raw cosines on a screenshot corpus pack into 0.9..1.0 (everything is a
 * picture of a screen), so printing them directly reads as "97% vs 93% =
 * both certain". The percent shown here is therefore rescaled from the
 * z-score -- standard deviations above this result set's mean, computed in
 * `search_similar` -- so the best hit reads ~100 and the pack spreads below
 * it. The tooltip keeps the raw cosine for debugging. Bar and label always
 * come from the same value.
 */
function similarityTone(z: number) {
  if (z >= 1.0)
    return {
      bar: "bg-emerald-500",
      text: "text-emerald-600 dark:text-emerald-400",
    }
  if (z >= -0.5)
    return { bar: "bg-amber-500", text: "text-amber-600 dark:text-amber-400" }
  return { bar: "bg-rose-500", text: "text-rose-600 dark:text-rose-400" }
}

function zToPercent(z: number) {
  const clamped = Math.max(-2, Math.min(2, z))
  return Math.round(((clamped + 2) / 4) * 100)
}

function ScoreBar({
  score,
  scoreZ,
  tone = "card",
}: {
  score: number
  scoreZ?: number | null
  tone?: "card" | "overlay"
}) {
  // Rescaled from the z-score so the set spreads 0..100 instead of 93..97.
  // No z (single hit, zero spread) falls back to the raw cosine percent.
  const percent = scoreZ == null ? Math.round(score * 100) : zToPercent(scoreZ)
  const { bar, text } = similarityTone(scoreZ ?? 0)
  const track = tone === "overlay" ? "bg-white/15" : "bg-black/10"
  const fill = tone === "overlay" ? "bg-white/90" : bar
  return (
    <span className="flex items-center gap-1.5">
      <span
        title={`Cosine similarity ${score.toFixed(3)} — best in this set reads 100`}
        className={`h-1.5 w-12 overflow-hidden rounded-full ${track}`}
      >
        <span
          className={`block h-full rounded-full ${fill} transition-[width] duration-200`}
          style={{ width: `${percent}%` }}
        />
      </span>
      <span className={`text-[11px] font-semibold tabular-nums ${text}`}>
        {percent}%
      </span>
    </span>
  )
}

function DetailStatus({
  dot,
  label,
  value,
}: {
  dot: string
  label: string
  value: string
}) {
  return (
    <div className="flex items-center gap-2 text-xs">
      <span className={`size-1.5 shrink-0 rounded-full ${dot}`} />
      <span className="text-white/50">{label}</span>
      <span className="truncate font-medium text-white/90">{value}</span>
    </div>
  )
}

function ShotDetails({
  path,
  score,
  scoreZ,
}: {
  path: string
  score?: number | null
  scoreZ?: number | null
}) {
  const [open, setOpen] = useState(false)
  const [detail, setDetail] = useState<ScreenshotDetail | null>(null)
  const [failed, setFailed] = useState(false)
  const [copied, setCopied] = useState(false)

  useEffect(() => {
    if (!open || detail || failed) return
    let cancelled = false
    screenshotDetail(path)
      .then((d) => {
        if (!cancelled) setDetail(d)
      })
      .catch(() => {
        if (!cancelled) setFailed(true)
      })
    return () => {
      cancelled = true
    }
  }, [open, path, detail, failed])

  async function copyText() {
    if (!detail?.ocr_text) return
    try {
      await navigator.clipboard.writeText(detail.ocr_text)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      setFailed(true)
    }
  }

  return (
    <div className="shrink-0 border-t border-white/10">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="flex w-full items-center gap-2 px-4 py-2.5 text-left text-xs font-medium text-white/80 transition-colors outline-none hover:text-white focus-visible:text-white"
      >
        <CaretDown
          size={14}
          className={`shrink-0 transition-transform duration-200 ${open ? "" : "-rotate-90"}`}
        />
        Details
        {detail ? (
          <span className="font-normal text-white/40">
            {detail.ocr_word_count === 1
              ? "1 word"
              : `${detail.ocr_word_count} words`}
            {detail.embed_model && detail.embed_model !== "skipped"
              ? " · indexed"
              : ""}
          </span>
        ) : null}
        {score != null ? (
          <span className="ml-auto">
            <ScoreBar score={score} scoreZ={scoreZ} tone="overlay" />
          </span>
        ) : null}
      </button>
      <div
        className={`grid transition-[grid-template-rows] duration-200 ease-out ${open ? "grid-rows-[1fr]" : "grid-rows-[0fr]"}`}
      >
        <div className="overflow-hidden">
          <div className="grid gap-4 px-4 pb-4 sm:grid-cols-[1fr_220px]">
            <div className="min-w-0">
              <div className="mb-1.5 flex items-center gap-2">
                <p className="text-[11px] font-medium tracking-wide text-white/40 uppercase">
                  Scanned text
                </p>
                {detail?.ocr_text ? (
                  <Button
                    size="icon-xs"
                    variant="ghost"
                    title={copied ? "Copied" : "Copy text"}
                    className="text-white/60 hover:bg-white/10 hover:text-white"
                    onClick={copyText}
                  >
                    {copied ? <Check size={12} /> : <Copy size={12} />}
                  </Button>
                ) : null}
              </div>
              {failed ? (
                <p className="text-xs text-white/50">
                  Could not load details for this screenshot.
                </p>
              ) : !detail ? (
                <div className="space-y-1.5" aria-label="Loading details">
                  <div className="h-3 animate-pulse rounded bg-white/10" />
                  <div className="h-3 w-4/5 animate-pulse rounded bg-white/10" />
                  <div className="h-3 w-3/5 animate-pulse rounded bg-white/10" />
                </div>
              ) : !detail.ocr_indexed ? (
                <p className="text-xs text-white/50">
                  Not indexed yet — text will appear once background indexing
                  reaches it.
                </p>
              ) : detail.ocr_text ? (
                <p className="max-h-44 overflow-auto rounded-lg bg-white/5 p-2.5 text-xs leading-relaxed whitespace-pre-wrap text-white/85">
                  {detail.ocr_text}
                </p>
              ) : (
                <p className="text-xs text-white/50">
                  No readable text found in this screenshot.
                </p>
              )}
            </div>
            <div className="space-y-2">
              <p className="text-[11px] font-medium tracking-wide text-white/40 uppercase">
                Index
              </p>
              {detail ? (
                <>
                  <DetailStatus
                    dot={detail.ocr_indexed ? "bg-emerald-400" : "bg-amber-400"}
                    label="Text"
                    value={
                      !detail.ocr_indexed
                        ? "pending"
                        : detail.ocr_word_count === 0
                          ? "none found"
                          : `${detail.ocr_word_count} words`
                    }
                  />
                  <DetailStatus
                    dot={
                      detail.embedded_at
                        ? "bg-emerald-400"
                        : detail.embed_model === "skipped"
                          ? "bg-amber-400"
                          : "bg-white/30"
                    }
                    label="Visual"
                    value={
                      detail.embedded_at
                        ? (detail.embed_model ?? "indexed")
                        : detail.embed_model === "skipped"
                          ? "model loading"
                          : "pending"
                    }
                  />
                </>
              ) : (
                <div className="space-y-1.5" aria-label="Loading index state">
                  <div className="h-3 w-2/3 animate-pulse rounded bg-white/10" />
                  <div className="h-3 w-1/2 animate-pulse rounded bg-white/10" />
                </div>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

export function ShotPreview({
  item,
  score,
  scoreZ,
  onClose,
  onOpenFolder,
}: {
  item: Screenshot | null
  score?: number | null
  scoreZ?: number | null
  onClose: () => void
  onOpenFolder?: () => void
}) {
  const [loaded, setLoaded] = useState<{ path: string; url: string } | null>(
    null
  )
  const [failedPath, setFailedPath] = useState<string | null>(null)

  useEffect(() => {
    if (!item) return
    let cancelled = false
    getThumbnail(item.path, PREVIEW_SIZE)
      .then((url) => {
        if (!cancelled) setLoaded({ path: item.path, url })
      })
      .catch(() => {
        if (!cancelled) setFailedPath(item.path)
      })
    return () => {
      cancelled = true
    }
  }, [item])

  const full = item && loaded?.path === item.path ? loaded.url : null
  const failed = item != null && failedPath === item.path

  useEffect(() => {
    if (!item) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose()
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [item, onClose])

  if (!item) return null

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={fileName(item.path)}
      className="fixed inset-0 z-50 flex flex-col bg-black/80 p-4 backdrop-blur-md sm:p-8"
      onClick={onClose}
    >
      <div
        className="mx-auto flex min-h-0 w-full max-w-4xl flex-col overflow-hidden rounded-2xl border border-white/10 bg-neutral-900 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <header className="flex shrink-0 items-center gap-2 px-4 py-3">
          <div className="min-w-0 flex-1">
            <p className="truncate text-sm font-medium text-white">
              {fileName(item.path)}
            </p>
            <p className="text-xs text-white/50">
              {new Date(item.created_at).toLocaleString()} · {item.width}×
              {item.height} · {item.source}
            </p>
          </div>
          <Button
            size="icon-sm"
            variant="ghost"
            title="Open in image viewer"
            className="text-white/70 hover:bg-white/10 hover:text-white"
            onClick={() => {
              openFile(item.path).catch(() => undefined)
            }}
          >
            <ArrowsOut />
          </Button>
          {onOpenFolder ? (
            <Button
              size="icon-sm"
              variant="ghost"
              title="Open save folder"
              className="text-white/70 hover:bg-white/10 hover:text-white"
              onClick={onOpenFolder}
            >
              <FolderOpen />
            </Button>
          ) : null}
          <Button
            size="icon-sm"
            variant="ghost"
            title="Close (Esc)"
            className="text-white/70 hover:bg-white/10 hover:text-white"
            onClick={onClose}
          >
            <X />
          </Button>
        </header>

        <div className="grid min-h-0 flex-1 place-items-center overflow-auto bg-black/40 p-4">
          {full ? (
            <img
              src={full}
              alt={fileName(item.path)}
              className="max-h-[55vh] max-w-full rounded-lg object-contain shadow-2xl"
            />
          ) : (
            <p className="text-sm text-white/60">
              {failed ? "Could not load the image." : "Loading full size…"}
            </p>
          )}
        </div>

        <ShotDetails
          key={item.path}
          path={item.path}
          score={score}
          scoreZ={scoreZ}
        />
      </div>
    </div>
  )
}

export function ShotCard({
  item,
  thumb,
  snippet,
  score,
  scoreZ,
  onOpen,
  onOpenFolder,
  onDelete,
}: {
  item: Screenshot
  thumb?: string
  snippet?: string | null
  score?: number | null
  scoreZ?: number | null
  onOpen: (item: Screenshot) => void
  onOpenFolder?: () => void
  onDelete?: (item: Screenshot) => void
}) {
  const name = fileName(item.path)
  return (
    <figure className="group overflow-hidden rounded-xl border bg-card transition-[transform,box-shadow,border-color] duration-150 focus-within:border-primary/40 hover:-translate-y-0.5 hover:border-primary/40 hover:shadow-lg">
      <button
        type="button"
        onClick={() => onOpen(item)}
        title={`View ${name}`}
        className="relative block w-full cursor-zoom-in outline-none"
      >
        {thumb ? (
          <img
            src={thumb}
            alt={name}
            className="aspect-video w-full bg-muted object-cover"
            loading="lazy"
          />
        ) : (
          <div className="grid aspect-video animate-pulse place-items-center bg-muted">
            <span className="text-xs text-muted-foreground">…</span>
          </div>
        )}
        <span className="absolute inset-0 bg-black/25 opacity-0 transition-opacity duration-150 group-hover:opacity-100" />
        <span className="absolute inset-x-0 bottom-0 flex items-end justify-between gap-1 p-2 opacity-0 transition-opacity duration-150 group-hover:opacity-100">
          <span className="rounded-md bg-black/70 px-1.5 py-0.5 text-[11px] text-white tabular-nums">
            {item.width}×{item.height}
          </span>
          <span className="rounded-md bg-black/70 p-1 text-white">
            <ArrowsOut size={12} />
          </span>
        </span>
        {score != null ? (
          <span className="absolute top-2 right-2 rounded-full bg-black/70 px-2 py-0.5 text-[11px] font-medium text-white tabular-nums">
            {scoreZ != null
              ? `${zToPercent(scoreZ)}%`
              : `${Math.round(score * 100)}%`}
          </span>
        ) : null}
      </button>
      <figcaption className="space-y-1 p-2.5">
        <div className="flex items-center gap-1.5">
          <div className="min-w-0 flex-1">
            <p className="truncate text-xs font-medium">{name}</p>
            <p className="text-[11px] text-muted-foreground">
              {new Date(item.created_at).toLocaleString()}
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
            {onOpenFolder ? (
              <Button
                size="icon-xs"
                variant="ghost"
                title="Open save folder"
                onClick={onOpenFolder}
              >
                <FolderOpen />
              </Button>
            ) : null}
            {onDelete ? (
              <Button
                size="icon-xs"
                variant="ghost"
                title="Delete screenshot and its file"
                className="hover:bg-destructive/10 hover:text-destructive"
                onClick={() => onDelete(item)}
              >
                <Trash />
              </Button>
            ) : null}
          </div>
        </div>
        {snippet ? (
          <p className="line-clamp-2 text-[11px] leading-snug text-muted-foreground">
            <Snippet text={snippet} />
          </p>
        ) : score != null ? (
          <div className="pt-0.5">
            <ScoreBar score={score} scoreZ={scoreZ} />
          </div>
        ) : null}
      </figcaption>
    </figure>
  )
}
