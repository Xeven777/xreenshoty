export { cn } from "cn"

export function fileName(path: string) {
  return path.split(/[/\\]/).pop() ?? path
}

export function dirOf(path: string) {
  const i = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"))
  return i > 0 ? path.slice(0, i) : path
}
