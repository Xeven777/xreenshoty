import { memo, useCallback, useEffect, useState } from "react"
import {
  ArrowsOut,
  CaretDown,
  Check,
  Copy,
  FolderOpen,
  Trash,
  X,
} from "@phosphor-icons/react"
import { AnimatePresence, motion } from "motion/react"

import { Button } from "@/components/ui/button"
import {
  getThumbnail,
  openFile,
  screenshotDetail,
  type Screenshot,
  type ScreenshotDetail,
} from "@/lib/screenshot"
import { toastCopied, toastFailed } from "@/lib/toast"
import { fileName } from "@/lib/utils"
import { getCachedPreview, setCachedPreview } from "@/lib/use-thumbnails"

const PREVIEW_SIZE = 1600
const START = "\u001f"
const END = "\u001e"

export function usePreview() {
  const [item, setItem] = useState<Screenshot | null>(null)
  const hide = useCallback(() => setItem(null), [])
  return { item, show: setItem, hide }
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

function scorePercent(score: number, scoreZ?: number | null) {
  return scoreZ == null ? Math.round(score * 100) : zToPercent(scoreZ)
}

function formatDate(created: number) {
  return new Date(created).toLocaleString()
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
  const percent = scorePercent(score, scoreZ)
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
      toastCopied()
      setTimeout(() => setCopied(false), 1500)
    } catch (e) {
      setFailed(true)
      toastFailed("Copy failed", e)
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
  onOpenFolder?: (item: Screenshot) => void
}) {
  const [loaded, setLoaded] = useState<{ path: string; url: string } | null>(
    null
  )
  const [failedPath, setFailedPath] = useState<string | null>(null)

  useEffect(() => {
    if (!item) return
    // Cache hits are picked up during render; only a miss fetches.
    if (getCachedPreview(item.path)) return
    let cancelled = false
    getThumbnail(item.path, PREVIEW_SIZE)
      .then((url) => {
        if (cancelled) return
        setCachedPreview(item.path, url)
        setLoaded({ path: item.path, url })
      })
      .catch(() => {
        if (!cancelled) setFailedPath(item.path)
      })
    return () => {
      cancelled = true
    }
  }, [item])

  const cachedUrl = item ? getCachedPreview(item.path) : undefined
  const full =
    item && loaded?.path === item.path ? loaded.url : (cachedUrl ?? null)
  const failed = item != null && failedPath === item.path

  useEffect(() => {
    if (!item) return
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose()
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [item, onClose])

  return (
    <AnimatePresence>
      {item ? (
        <motion.div
          key={item.path}
          role="dialog"
          aria-modal="true"
          aria-label={fileName(item.path)}
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          // Exit is deliberately faster than the enter: the backdrop's job on
          // close is to get out of the way, not to keep the moment going.
          exit={{
            opacity: 0,
            transition: { duration: 0.12, ease: [0.4, 0, 1, 1] },
          }}
          transition={{ duration: 0.18 }}
          className="fixed inset-0 z-50 flex flex-col bg-black/80 p-4 sm:p-8"
          onClick={onClose}
        >
          <motion.div
            initial={{ opacity: 0, scale: 0.97, y: 10 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{
              opacity: 0,
              scale: 0.98,
              y: 6,
              // Snappier than the enter spring — arriving gets the spring,
              // leaving gets 120ms.
              transition: { duration: 0.12, ease: [0.4, 0, 1, 1] },
            }}
            transition={{ type: "spring", stiffness: 380, damping: 32 }}
            className="mx-auto flex min-h-0 w-full max-w-4xl flex-col overflow-hidden rounded-2xl border border-white/10 bg-neutral-900 shadow-2xl"
            onClick={(e) => e.stopPropagation()}
          >
            <header className="flex shrink-0 items-center gap-2 px-4 py-3">
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium text-white">
                  {fileName(item.path)}
                </p>
                <p className="text-xs text-white/50">
                  {formatDate(item.created_at)} · {item.width}×{item.height} ·{" "}
                  {item.source}
                </p>
              </div>
              <Button
                size="icon-sm"
                variant="ghost"
                title="Open in image viewer"
                className="text-white/70 hover:bg-white/10 hover:text-white"
                onClick={() => {
                  openFile(item.path).catch((e) =>
                    toastFailed("Couldn't open image", e)
                  )
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
                  onClick={() => onOpenFolder(item)}
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
                <motion.img
                  key={item.path}
                  src={full}
                  alt={fileName(item.path)}
                  initial={{ opacity: 0, scale: 0.98 }}
                  animate={{ opacity: 1, scale: 1 }}
                  transition={{ duration: 0.2 }}
                  decoding="async"
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
          </motion.div>
        </motion.div>
      ) : null}
    </AnimatePresence>
  )
}

export const ShotCard = memo(function ShotCard({
  item,
  thumb,
  index = 0,
  snippet,
  score,
  scoreZ,
  onOpen,
  onOpenFolder,
  onDelete,
}: {
  item: Screenshot
  thumb?: string
  /** Position in the rendered list; drives the entrance stagger only. */
  index?: number
  snippet?: string | null
  score?: number | null
  scoreZ?: number | null
  onOpen: (item: Screenshot) => void
  onOpenFolder?: (item: Screenshot) => void
  onDelete?: (item: Screenshot) => void
}) {
  const name = fileName(item.path)
  return (
    <motion.figure
      // No `layout` here: the grid uses `content-visibility`, so scroll-in
      // hands the card a real box where the intrinsic placeholder was, and a
      // layout animation would size-shift every card mid-scroll. Removal is
      // animated by the grid's `popLayout` + this exit instead.
      exit={{ opacity: 0, scale: 0.96 }}
      initial={{ opacity: 0, y: 8 }}
      whileInView={{ opacity: 1, y: 0 }}
      viewport={{ once: true, margin: "80px" }}
      transition={{
        duration: 0.22,
        ease: "easeOut",
        // Cascade the first few cards so a settled search does not drop the
        // whole grid in on one frame. Capped — decorative, never blocking.
        delay: Math.min(index, 8) * 0.04,
      }}
      className="group overflow-hidden rounded-xl border bg-card transition-[transform,box-shadow,border-color] duration-150 focus-within:border-primary/40 hover:-translate-y-0.5 hover:border-primary/40 hover:shadow-lg"
    >
      <button
        type="button"
        onClick={() => onOpen(item)}
        title={`View ${name}`}
        className="relative block w-full cursor-zoom-in transition-transform duration-150 outline-none active:scale-[0.98]"
      >
        {thumb ? (
          <img
            src={thumb}
            alt={name}
            decoding="async"
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
            {scorePercent(score, scoreZ)}%
          </span>
        ) : null}
      </button>
      <figcaption className="space-y-1 p-2.5">
        <div className="flex items-center gap-1.5">
          <div className="min-w-0 flex-1">
            <p className="truncate text-xs font-medium">{name}</p>
            <p className="text-[11px] text-muted-foreground">
              {formatDate(item.created_at)}
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
            {onOpenFolder ? (
              <Button
                size="icon-xs"
                variant="ghost"
                title="Open save folder"
                onClick={() => onOpenFolder(item)}
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
    </motion.figure>
  )
})
