import { toast } from "sonner"

import { fileName } from "./utils"

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

export { toast }
