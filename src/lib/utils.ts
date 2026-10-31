export { cn } from "cn"

export function fileName(path: string) {
  return path.split(/[/\\]/).pop() ?? path
}

export function dirOf(path: string) {
  const i = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"))
  return i > 0 ? path.slice(0, i) : path
}

/** `plural(1, "face")` → `1 face`, `plural(3, "face")` → `3 faces`. */
export function plural(n: number, one: string, many = `${one}s`) {
  return `${n} ${n === 1 ? one : many}`
}

/** Human-readable byte count for cache sizes. */
export function formatBytes(bytes: number) {
  if (bytes <= 0) return "0 B"
  const units = ["B", "KB", "MB", "GB"]
  const exp = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)))
  const value = bytes / 1024 ** exp
  return `${exp === 0 ? value : value.toFixed(1)} ${units[exp]}`
}
