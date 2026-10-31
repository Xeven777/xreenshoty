import { toast } from "sonner"

import { fileName, formatBytes, plural } from "./utils"

/** Consistent in-app toast copy. OS notifications stay in `notify.ts` for
 *  background saves; these are for foreground feedback that survives tab
 *  switches (unlike the Capture status banner). */

export function toastSaved(path: string, onOpenFolder: () => void) {
  toast.success("Screenshot saved", {
    description: fileName(path),
    action: { label: "Open folder", onClick: onOpenFolder },
  })
}

export function toastSavedMany(count: number, latest: string) {
  toast.success(`Saved ${count} screenshots`, { description: fileName(latest) })
}

export function toastDeleted(path: string) {
  toast.success("Deleted", { description: fileName(path) })
}

export function errorText(error: unknown) {
  return String(error instanceof Error ? error.message : error)
}

export function toastFailed(title: string, error: unknown, retry?: () => void) {
  toast.error(title, {
    description: errorText(error),
    action: retry ? { label: "Retry", onClick: retry } : undefined,
  })
}

export function toastCopied() {
  toast.success("Copied to clipboard")
}

export function toastCacheCleared(bytes: number) {
  toast.success("Thumbnail cache cleared", {
    description: `${formatBytes(bytes)} freed — thumbnails rebuild as you scroll`,
  })
}

/**
 * Result of a manual regroup. Reclustering is sub-millisecond, so without this
 * the button looks like it did nothing.
 */
export function toastReclustered(written: number, people: number) {
  if (written === 0) {
    toast("Nothing to group yet", {
      description:
        "No faces detected so far, or every face has only been seen once.",
    })
    return
  }
  toast.success("People regrouped", {
    description: `${plural(written, "face")} in ${plural(people, "person", "people")}.`,
  })
}

/** A person removed from People. Detection is not re-run for them, so this is
 *  final until the face model changes. */
export function toastForgotten(faces: number) {
  toast.success("Person removed", {
    description: `${plural(faces, "face")} left the index. Screenshots are untouched.`,
  })
}

export { toast }
