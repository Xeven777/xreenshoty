import { trackedInvoke } from "./tauri"

export interface MonitorInfo {
  id: number
  x: number
  y: number
  width: number
  height: number
}

export function listMonitors() {
  return trackedInvoke<MonitorInfo[]>("list_monitors")
}

export function getDefaultSaveDir() {
  return trackedInvoke<string>("default_save_dir")
}

export function captureFullscreen(
  monitorId: number | null,
  saveDir: string | null
) {
  return trackedInvoke<string[]>("capture_fullscreen", {
    monitorId,
    saveDir,
  })
}

export function captureRegion(
  x: number,
  y: number,
  width: number,
  height: number,
  saveDir: string | null
) {
  return trackedInvoke<string>("capture_region", {
    x,
    y,
    width,
    height,
    saveDir,
  })
}

export function captureActiveWindow(saveDir: string | null) {
  return trackedInvoke<string>("capture_active_window", { saveDir })
}

/** Hand the original PNG to the system image viewer. */
export function openFile(path: string) {
  return trackedInvoke<void>("open_file", { path })
}

export function openFolder(path: string | null) {
  return trackedInvoke<void>("open_folder", { path })
}

/** Downscaled PNG as a data URL, generated in Rust so any folder works. */
export function getThumbnail(path: string, size = 320) {
  return trackedInvoke<string>("thumbnail", { path, size })
}

/** Hide the main window, grab the monitor under the cursor, open the drag overlay. */
export function beginRegionSelect() {
  return trackedInvoke<void>("begin_region_select")
}

/** Close the overlay if still open. */
export function closeRegionOverlay() {
  return trackedInvoke<void>("close_region_overlay")
}

/** Show the main window again once the overlay is gone. */
export function restoreMainWindow() {
  return trackedInvoke<void>("restore_main_window")
}

/** Crop the selected rect out of the frozen backdrop and save it. */
export function saveRegionFromBackdrop(
  x: number,
  y: number,
  width: number,
  height: number,
  saveDir: string | null,
) {
  return trackedInvoke<string>("save_region_from_backdrop", {
    x,
    y,
    width,
    height,
    saveDir,
  })
}

export interface Screenshot {
  path: string
  created_at: number
  width: number
  height: number
  source: string
}

/** Most recently captured or imported screenshots, newest first. */
export function listRecent(limit = 60) {
  return trackedInvoke<Screenshot[]>("list_recent", { limit })
}

/** Index PNGs already in the save folder. Idempotent. */
export function importExisting(dir: string | null) {
  return trackedInvoke<number>("import_existing", { dir })
}

/** Keyword search over OCR text. The query is sanitised Rust-side, because FTS5
 *  treats `-`, `"`, `*` and `:` as operators and would reject raw input. */
export function searchText(query: string, limit = 50) {
  return trackedInvoke<Screenshot[]>("search_text", { query, limit })
}

/** Semantic search: embeds the query and returns the nearest screenshots by
 *  cosine distance. Requires the embedding server to be running. */
export function searchSimilar(query: string, limit = 50) {
  return trackedInvoke<Screenshot[]>("search_similar", { query, limit })
}

export interface IndexStatus {
  embedded: number
  ocred: number
  total: number
  paused: boolean
  server_ready: boolean
}

export function indexStatus() {
  return trackedInvoke<IndexStatus>("index_status")
}

export function indexPause(paused: boolean) {
  return trackedInvoke<void>("index_pause", { paused })
}

export function screenshotCount() {
  return trackedInvoke<number>("screenshot_count")
}
