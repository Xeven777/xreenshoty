import { emit } from "@tauri-apps/api/event"
import { invoke, isTauri, type InvokeArgs } from "@tauri-apps/api/core"

import {
  emitDebugEvent,
  serializeDebugValue,
  serializeError,
} from "./debug-events"

const isDev = import.meta.env.DEV

/** Payloads that would flood the debug log — thumbnails and backdrops are
 *  50–150 KB base64 each; cloning them through JSON per image stalls the
 *  history grid. */
const HEAVY_COMMANDS = new Set(["thumbnail", "backdrop_data_url"])

function debugResult(command: string, result: unknown): unknown {
  if (HEAVY_COMMANDS.has(command) && typeof result === "string") {
    return `<${(result.length / 1024).toFixed(0)}KB data-url>`
  }
  return serializeDebugValue(result)
}

export async function trackedInvoke<T>(command: string, args?: InvokeArgs) {
  const startedAt = performance.now()

  try {
    const result = await invoke<T>(command, args)

    if (isDev) {
      emitDebugEvent({
        id: crypto.randomUUID(),
        kind: "invoke",
        command,
        args: serializeDebugValue(args),
        durationMs: Math.round((performance.now() - startedAt) * 100) / 100,
        status: "success",
        result: debugResult(command, result),
        timestamp: new Date().toISOString(),
      })
    }

    return result
  } catch (error) {
    if (isDev) {
      emitDebugEvent({
        id: crypto.randomUUID(),
        kind: "invoke",
        command,
        args: serializeDebugValue(args),
        durationMs: Math.round((performance.now() - startedAt) * 100) / 100,
        status: "error",
        error: serializeError(error),
        timestamp: new Date().toISOString(),
      })
    }

    throw error
  }
}

export async function trackedEmit<T>(event: string, payload?: T) {
  await emit(event, payload)

  emitDebugEvent({
    id: crypto.randomUUID(),
    kind: "runtime-event",
    name: event,
    source: "tauri-event",
    payload: serializeDebugValue(payload),
    timestamp: new Date().toISOString(),
  })
}

export { isTauri }
