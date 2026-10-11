import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification"

let permission: boolean | null = null

/** Best-effort desktop notification. Silently no-ops without a notification daemon. */
export async function notify(title: string, body: string) {
  try {
    if (permission === null) {
      permission =
        (await isPermissionGranted()) ||
        (await requestPermission()) === "granted"
    }
    if (!permission) return
    sendNotification({ title, body })
  } catch {
    /* no notification daemon available */
  }
}
