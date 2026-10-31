import { useEffect, useMemo, useRef, useState } from "react"

import { getFaceThumbnail } from "@/lib/screenshot"

/** Face tiles are decoded + cropped + resized in Rust, so requests are pooled
 *  rather than fired all at once — same reasoning as `use-thumbnails`. */
const CONCURRENCY = 6

/** Base64 crops are small, but cap the map so a long session does not pin them
 *  forever. */
const MAX_CACHED = 200

const cache = new Map<number, string>()

function cacheSet(faceId: number, url: string) {
  cache.delete(faceId)
  cache.set(faceId, url)
  while (cache.size > MAX_CACHED) {
    const oldest = cache.keys().next().value
    if (oldest === undefined) break
    cache.delete(oldest)
  }
}

/** See `clearMemoryCaches`: the disk cache clear is a no-op without this. */
export function clearFaceThumbnailCache() {
  cache.clear()
}

/**
 * Loads face crops for `faceIds`, skipping any already loaded. Shared by every
 * place that shows a person tile.
 */
export function useFaceThumbnails(faceIds: number[]) {
  const [thumbs, setThumbs] = useState<Record<number, string>>({})
  const seenRef = useRef<Set<number>>(new Set())

  const key = useMemo(() => faceIds.join(","), [faceIds])

  useEffect(() => {
    const wanted = key ? key.split(",").map(Number) : []
    const cached: Record<number, string> = {}
    const missing: number[] = []
    for (const id of wanted) {
      if (seenRef.current.has(id)) continue
      const hit = cache.get(id)
      if (hit) cached[id] = hit
      else missing.push(id)
    }
    if (Object.keys(cached).length === 0 && missing.length === 0) return
    let cancelled = false

    void (async () => {
      const queue = [...missing]
      const loaded: Record<number, string> = {}

      const worker = async () => {
        for (let id = queue.shift(); id !== undefined; id = queue.shift()) {
          try {
            const url = await getFaceThumbnail(id)
            cacheSet(id, url)
            loaded[id] = url
          } catch {
            /* leave the placeholder */
          }
        }
      }
      await Promise.all(
        Array.from({ length: Math.min(CONCURRENCY, queue.length) }, worker)
      )
      if (cancelled) return

      for (const id of [...Object.keys(cached), ...Object.keys(loaded)]) {
        seenRef.current.add(Number(id))
      }
      setThumbs((prev) => ({ ...prev, ...cached, ...loaded }))
    })()

    return () => {
      cancelled = true
    }
    // `thumbs` is what this writes, so it must not be a dep; `key` covers the
    // input changing instead.
  }, [key])

  return thumbs
}
