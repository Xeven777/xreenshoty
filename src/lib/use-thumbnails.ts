import { useEffect, useState } from "react"

import { getThumbnail } from "@/lib/screenshot"

/** Thumbnails are generated in Rust (a full decode + resize + PNG encode each),
 *  so requests are pooled rather than fired all at once. Six at a time keeps the
 *  grid filling without starving the backend of everything else. */
const CONCURRENCY = 6

/**
 * Loads thumbnails for `paths`, skipping any already loaded.
 *
 * Shared by the history grid and search results: both render the same cards and
 * both would otherwise carry their own copy of this loader.
 */
export function useThumbnails(paths: string[]) {
  const [thumbs, setThumbs] = useState<Record<string, string>>({})

  // Value-keyed so a new array identity with the same contents does not refetch.
  const key = paths.join("\n")

  useEffect(() => {
    const wanted = key ? key.split("\n") : []
    const missing = wanted.filter((p) => !(p in thumbs))
    if (missing.length === 0) return
    let cancelled = false

    void (async () => {
      const queue = [...missing]
      const loaded: Record<string, string> = {}

      const worker = async () => {
        for (let path = queue.shift(); path; path = queue.shift()) {
          try {
            loaded[path] = await getThumbnail(path)
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
      // this very effect.
      setThumbs((prev) => ({ ...prev, ...loaded }))
    })()

    return () => {
      cancelled = true
    }
    // `thumbs` is intentionally excluded: it is what this writes, so depending
    // on it would loop. `key` covers the input changing instead.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key])

  return thumbs
}
