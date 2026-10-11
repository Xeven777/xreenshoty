import { useEffect, useState } from "react"
import { ArrowsOut, FolderOpen, X } from "@phosphor-icons/react"

import { Button } from "@/components/ui/button"
import { getThumbnail, openFile, type Screenshot } from "@/lib/screenshot"

export function fileName(path: string) {
  return path.split(/[/\\]/).pop() ?? path
}

/** Large preview is generated on demand; the grid thumbnails are far too
 *  small to read, and re-encoding 60 of them up front would be wasteful. */
const PREVIEW_SIZE = 1600

/** Which screenshot is open in the full-size viewer, if any. */
export function usePreview() {
  const [item, setItem] = useState<Screenshot | null>(null)
  return { item, show: setItem, hide: () => setItem(null) }
}

/**
 * Full-size viewer. Click the backdrop or press Escape to dismiss.
 */
export function ShotPreview({
  item,
  onClose,
  onOpenFolder,
}: {
  item: Screenshot | null
  onClose: () => void
  onOpenFolder?: () => void
}) {
  // Results are tagged with the path they belong to, so a new selection
  // invalidates them by derivation rather than by resetting state in an
  // effect (which lint rightly rejects, and which would flash the old image).
  const [loaded, setLoaded] = useState<{ path: string; url: string } | null>(null)
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
      className="fixed inset-0 z-50 flex flex-col bg-black/85 p-4 backdrop-blur-sm"
      onClick={onClose}
    >
      <header
        className="mb-3 flex shrink-0 items-center gap-2 text-white"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium">{fileName(item.path)}</p>
          <p className="text-xs text-white/60">
            {new Date(item.created_at).toLocaleString()} · {item.width}×
            {item.height} · {item.source}
          </p>
        </div>
        <Button
          size="icon-sm"
          variant="ghost"
          title="Open in image viewer"
          className="text-white hover:bg-white/15 hover:text-white"
          onClick={(e) => {
            e.stopPropagation()
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
            className="text-white hover:bg-white/15 hover:text-white"
            onClick={(e) => {
              e.stopPropagation()
              onOpenFolder()
            }}
          >
            <FolderOpen />
          </Button>
        ) : null}
        <Button
          size="icon-sm"
          variant="ghost"
          title="Close (Esc)"
          className="text-white hover:bg-white/15 hover:text-white"
          onClick={(e) => {
            e.stopPropagation()
            onClose()
          }}
        >
          <X />
        </Button>
      </header>

      <div
        className="grid min-h-0 flex-1 place-items-center"
        onClick={(e) => e.stopPropagation()}
      >
        {full ? (
          <img
            src={full}
            alt={fileName(item.path)}
            className="max-h-full max-w-full rounded-lg object-contain shadow-2xl"
          />
        ) : (
          <p className="text-sm text-white/70">
            {failed ? "Could not load the image." : "Loading full size…"}
          </p>
        )}
      </div>
    </div>
  )
}

/**
 * One screenshot tile. Clicking opens the viewer; the folder button is kept
 * out of the way until hover so it cannot swallow the click.
 */
export function ShotCard({
  item,
  thumb,
  onOpen,
  onOpenFolder,
}: {
  item: Screenshot
  thumb?: string
  onOpen: (item: Screenshot) => void
  onOpenFolder?: () => void
}) {
  const name = fileName(item.path)
  return (
    <figure
      className="group overflow-hidden rounded-xl border bg-card hover:border-primary/50"
    >
      <button
        type="button"
        onClick={() => onOpen(item)}
        title={`View ${name}`}
        className="relative block w-full cursor-zoom-in"
      >
        {thumb ? (
          <img
            src={thumb}
            alt={name}
            className="aspect-video w-full bg-muted object-cover"
            loading="lazy"
          />
        ) : (
          <div className="grid aspect-video place-items-center bg-muted">
            <span className="text-xs text-muted-foreground">…</span>
          </div>
        )}
        <span className="absolute inset-0 bg-black/25 opacity-0 transition-opacity duration-150 group-hover:opacity-100" />
        <span className="absolute inset-x-0 bottom-0 flex items-end justify-between gap-1 p-2 opacity-0 transition-opacity duration-150 group-hover:opacity-100">
          <span className="rounded bg-black/70 px-1.5 py-0.5 text-[11px] text-white">
            {item.width}×{item.height}
          </span>
          <span className="rounded bg-black/70 p-1 text-white">
            <ArrowsOut size={12} />
          </span>
        </span>
      </button>
      <figcaption className="flex items-center gap-1.5 p-2">
        <div className="min-w-0 flex-1">
          <p className="truncate text-xs font-medium">{name}</p>
          <p className="text-[11px] text-muted-foreground">
            {new Date(item.created_at).toLocaleString()}
          </p>
        </div>
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
      </figcaption>
    </figure>
  )
}