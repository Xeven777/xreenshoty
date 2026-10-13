import { useEffect, useMemo, useRef, useState } from "react"

import { getThumbnail } from "@/lib/screenshot"

/** Thumbnails are generated in Rust (a full decode + resize + PNG encode each),
 *  so requests are pooled rather than fired all at once. Six at a time keeps the
 *  grid filling without starving the backend of everything else. */
const CONCURRENCY = 6

/** Base64 thumbnails are heavy — cap the shared cache so deleted screenshots
 *  and old searches do not pin megabytes forever. */
const MAX_CACHED = 120

const cache = new Map<string, string>()

function lruSet(
  cache: Map<string, string>,
  max: number,
  key: string,
  value: string
) {
  cache.delete(key)
  cache.set(key, value)
  while (cache.size > max) {
    const oldest = cache.keys().next().value
    if (oldest === undefined) break
    cache.delete(oldest)
  }
}

function cacheSet(path: string, url: string) {
  lruSet(cache, MAX_CACHED, path, url)
}

/** Full-size preview URLs, kept tiny (last 3) so reopening is instant. */
const MAX_PREVIEW = 3
const previewCache = new Map<string, string>()

export function getCachedPreview(path: string) {
  return previewCache.get(path)
}

export function setCachedPreview(path: string, url: string) {
  lruSet(previewCache, MAX_PREVIEW, path, url)
}

/**
 * Drop both in-memory caches.
 *
 * Clearing the disk cache alone is not enough: these module-level maps would
 * keep serving every thumbnail the session already loaded, so the button would
 * appear to do nothing. Mounted grids hold their own state, so they re-request
 * on the next path change; anything not on screen regenerates.
 */
export function clearMemoryCaches() {
  cache.clear()
  previewCache.clear()
}

/**
 * Loads thumbnails for `paths`, skipping any already loaded.
 *
 * Shared by the history grid and search results: both render the same cards and
 * both would otherwise carry their own copy of this loader.
 */
export function useThumbnails(paths: string[]) {
  const [thumbs, setThumbs] = useState<Record<string, string>>({})
  const seenRef = useRef<Set<string>>(new Set())

  // Value-keyed so a new array identity with the same contents does not refetch.
  const key = useMemo(() => paths.join("\n"), [paths])

  useEffect(() => {
    const wanted = key ? key.split("\n") : []
    const cached: Record<string, string> = {}
    const missing: string[] = []
    for (const p of wanted) {
      if (seenRef.current.has(p)) continue
      const hit = cache.get(p)
      if (hit) cached[p] = hit
      else missing.push(p)
    }
    if (Object.keys(cached).length === 0 && missing.length === 0) return
    let cancelled = false

    void (async () => {
      const queue = [...missing]
      const loaded: Record<string, string> = {}

      const worker = async () => {
        for (let path = queue.shift(); path; path = queue.shift()) {
          try {
            const url = await getThumbnail(path)
            cacheSet(path, url)
            loaded[path] = url
          } catch {
            /* leave the placeholder */
          }
        }
      }
      await Promise.all(
        Array.from({ length: Math.min(CONCURRENCY, queue.length) }, worker)
      )
      if (cancelled) return

      // One merge per batch: copying the map on every completion retriggers
      // this very effect. seenRef marks what has been merged so a re-run only
      // fetches what is genuinely new.
      for (const p of [...Object.keys(cached), ...Object.keys(loaded)]) {
        seenRef.current.add(p)
      }
      setThumbs((prev) => ({ ...prev, ...cached, ...loaded }))
    })()

    return () => {
      cancelled = true
    }
    // `thumbs` is intentionally not a dep: it is what this writes, so
    // depending on it would loop. `key` covers the input changing instead.
  }, [key])

  return thumbs
}
