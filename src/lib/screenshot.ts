import { trackedInvoke } from "./tauri"
import { toastFailed } from "./toast"

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

/** Delete a screenshot: the PNG on disk and its search entries. */
export function deleteScreenshot(path: string) {
  return trackedInvoke<void>("delete_screenshot", { path })
}

export function openFolder(path: string | null) {
  return trackedInvoke<void>("open_folder", { path })
}

export function openFolderSafe(path: string | null) {
  return openFolder(path).catch((e) => toastFailed("Couldn't open folder", e))
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
  saveDir: string | null
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

export interface SearchHit extends Screenshot {
  snippet: string | null
  score: number | null
  score_z: number | null
}

export interface ScreenshotDetail extends Screenshot {
  ocr_text: string | null
  ocr_word_count: number
  ocr_indexed: boolean
  embedded_at: number | null
  embed_model: string | null
}

/** Most recently captured or imported screenshots, newest first. */
export function listRecent(limit = 60) {
  return trackedInvoke<Screenshot[]>("list_recent", { limit })
}

/** Index PNGs already in the save folder. Idempotent. */
export function importExisting(dir: string | null) {
  return trackedInvoke<number>("import_existing", { dir })
}

/** Set the folders watched for images dropped in from outside the app, and
 *  index each one immediately. Returns how many are now watched — a path that
 *  is not an existing directory is skipped, so this can be fewer than asked. */
export function setScanFolders(dirs: string[]) {
  return trackedInvoke<number>("set_scan_folders", { dirs })
}

/** Bytes held by the on-disk thumbnail cache. */
export function thumbnailCacheSize() {
  return trackedInvoke<number>("thumbnail_cache_size")
}

/** Drop every cached thumbnail. Returns the bytes freed. Regenerated on demand
 *  from the original PNGs, so nothing is lost. */
export function clearThumbnailCache() {
  return trackedInvoke<number>("clear_thumbnail_cache")
}

/** Keyword search over OCR text. Returns hits with an FTS5 excerpt per row;
 *  match boundaries are marked with \u001f…\u001e for the UI to highlight. */
export function searchText(query: string, limit = 50) {
  return trackedInvoke<SearchHit[]>("search_text", { query, limit })
}

/** Semantic search: embeds the query and returns the nearest screenshots.
 *  Each hit carries a 0..1 similarity score, highest first. Requires the
 *  embedding server to be running. */
/** Hybrid search: BM25 over OCR text fused with k-NN over image vectors, so a
 *  single query draws on both. Falls back to keyword-only if the model is up
 *  but still loading, which is the normal case right after launch. */
export function searchHybrid(query: string, limit = 50) {
  return trackedInvoke<SearchHit[]>("search_hybrid", { query, limit })
}

export function searchSimilar(query: string, limit = 50) {
  return trackedInvoke<SearchHit[]>("search_similar", { query, limit })
}

/** Everything the index knows about one screenshot: full OCR text plus
 *  indexing state. Fetched lazily by the details view, never the grid. */
export function screenshotDetail(path: string) {
  return trackedInvoke<ScreenshotDetail>("screenshot_detail", { path })
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

/** Push the face-indexing opt-in to Rust. Off by default (Milestone 7). */
export function setFaceIndexing(enabled: boolean) {
  return trackedInvoke<void>("set_face_indexing", { enabled })
}

export interface FaceStatus {
  venv_ready: boolean
  models_present: boolean
  face_count: number
  /** Faces that landed in a cluster; the rest were seen once or too small. */
  clustered_faces: number
  cluster_count: number
  /** Screenshots whose face pass has finished, and how many exist. */
  screenshots_indexed: number
  screenshots_total: number
}

/** face-env + models + indexed face count, for the Settings readout. */
export function faceStatus() {
  return trackedInvoke<FaceStatus>("face_status")
}

export interface OcrStatus {
  /** The engine that reads new screenshots. */
  engine: string
  /** The ocr-env interpreter and the bundled script. */
  venv_ready: boolean
  /** The Indra OCR model files. */
  models_present: boolean
}

/** OCR readiness, for the Settings readout. */
export function ocrStatus() {
  return trackedInvoke<OcrStatus>("ocr_status")
}

/** One grouped person. `label` is the user's name (null = "Person N"). */
export interface FaceCluster {
  cluster_id: number
  label: string | null
  representative_face_id: number
  face_count: number
  screenshot_ids: number[]
}

/** All people clusters, largest first. */
export function faceClusters() {
  return trackedInvoke<FaceCluster[]>("face_clusters")
}

/** Re-run clustering at the current eps. Returns faces assigned. */
export function reclusterFaces() {
  return trackedInvoke<number>("recluster_faces")
}

/** Name a cluster; `null`/blank clears it. */
export function renameCluster(clusterId: number, label: string | null) {
  return trackedInvoke<void>("rename_cluster", { clusterId, label })
}

/** Merge `from` into `into`. Durable across reclusters. Returns faces moved. */
export function mergeClusters(from: number, into: number) {
  return trackedInvoke<number>("merge_clusters", { from, into })
}

/** Forget a person: their faces leave the index, their screenshots stay.
 *  Returns how many faces were removed. */
export function forgetCluster(clusterId: number) {
  return trackedInvoke<number>("forget_cluster", { clusterId })
}

/** One person's screenshots, newest first. */
export function clusterScreenshots(clusterId: number) {
  return trackedInvoke<Screenshot[]>("cluster_screenshots", { clusterId })
}

/** 7.6: screenshots whose faces are closest to a reference face. */
export function searchByFace(faceId: number, limit = 60) {
  return trackedInvoke<SearchHit[]>("search_by_face", { faceId, limit })
}

/** Cropped face tile as a data URL, for the People grid. */
export function getFaceThumbnail(faceId: number, size = 160) {
  return trackedInvoke<string>("face_thumbnail", { faceId, size })
}

export function screenshotCount() {
  return trackedInvoke<number>("screenshot_count")
}

/** Push-to-talk: records while held, transcribes with Whistle, resolves to text. */
export function voiceStart() {
  return trackedInvoke<void>("voice_start")
}

export function voiceStop() {
  return trackedInvoke<string>("voice_stop")
}

export function voiceCancel() {
  return trackedInvoke<void>("voice_cancel")
}
