import {
  isRegistered,
  register,
  unregister,
} from "@tauri-apps/plugin-global-shortcut"

const MODIFIER_KEYS = new Set([
  "Control",
  "Shift",
  "Alt",
  "Meta",
  "AltGraph",
  "CapsLock",
  "Fn",
  "Super",
  "Hyper",
])

const KEY_ALIASES: Record<string, string> = {
  " ": "Space",
  Escape: "Escape",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Enter: "Enter",
  Backspace: "Backspace",
  Tab: "Tab",
  Delete: "Delete",
  Insert: "Insert",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
}

/** Turn a keydown into a Tauri accelerator string, or null if not bindable. */
export function eventToAccelerator(event: KeyboardEvent): string | null {
  if (MODIFIER_KEYS.has(event.key)) return null

  const parts: string[] = []
  if (event.ctrlKey) parts.push("Control")
  if (event.altKey) parts.push("Alt")
  if (event.shiftKey) parts.push("Shift")
  if (event.metaKey) parts.push("Super")

  const raw = KEY_ALIASES[event.key] ?? event.key
  const isFunctionKey = /^F\d{1,2}$/.test(raw)
  const key = /^[a-zA-Z0-9]$/.test(raw) ? raw.toUpperCase() : raw

  // Bare keys would swallow normal typing, so require a modifier.
  if (parts.length === 0 && !isFunctionKey) return null

  parts.push(key)
  return parts.join("+")
}

export function prettyAccelerator(accel: string) {
  return accel
    .split("+")
    .map(
      (part) =>
        ({
          Control: "Ctrl",
          Alt: "Alt",
          Shift: "Shift",
          Super: "Super",
        })[part] ?? part
    )
    .join(" + ")
}

export async function shortcutAvailable(accel: string) {
  return !(await isRegistered(accel))
}

const chains = new Map<string, Promise<void>>()

/**
 * Run `op` after every earlier operation on the same accelerator. Bind and
 * release are each a pair of round trips (`isRegistered` then `register`), so
 * without this a bind that is still in flight when its effect is torn down
 * lands *after* the cleanup's release and holds the accelerator for the rest of
 * the session — and a late release can just as easily unregister an accelerator
 * a newer effect now owns (M2).
 */
function serialized<T>(accel: string, op: () => Promise<T>): Promise<T> {
  const previous = chains.get(accel) ?? Promise.resolve()
  const run = previous.then(op, op)
  const settled: Promise<void> = run.then(
    () => undefined,
    () => undefined
  )
  chains.set(accel, settled)
  void settled.then(() => {
    if (chains.get(accel) === settled) chains.delete(accel)
  })
  return run
}

/**
 * Register a global shortcut. Returns an error message when the accelerator is
 * already claimed by another app, so the UI can surface it.
 */
export function bindShortcut(accel: string, handler: () => void) {
  if (!accel) return Promise.resolve<string | null>(null)
  return serialized(accel, async () => {
    if (await isRegistered(accel)) {
      return `${prettyAccelerator(accel)} is already used by another app`
    }
    try {
      await register(accel, handler)
      return null
    } catch (e) {
      return `Could not register ${prettyAccelerator(accel)}: ${String(e)}`
    }
  })
}

export async function releaseShortcut(accel: string) {
  if (!accel) return
  await serialized(accel, async () => {
    try {
      if (await isRegistered(accel)) await unregister(accel)
    } catch {
      /* already gone */
    }
  })
}
