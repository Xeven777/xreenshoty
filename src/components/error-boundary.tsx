import { Component, type ErrorInfo, type ReactNode } from "react"
import { error as logError } from "@tauri-apps/plugin-log"

/**
 * Catches render-time exceptions so a crash shows what broke instead of an
 * empty window.
 *
 * Without this, a thrown error unmounts the entire tree; the document keeps the
 * app's own background, so on the dark theme that is indistinguishable from a
 * black screen — and nothing appears in the Rust log, because nothing failed
 * on the Rust side.
 */
export class ErrorBoundary extends Component<
  { children: ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null }

  static getDerivedStateFromError(error: Error) {
    return { error }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    void logError(
      `webview render error: ${error.message} (${info.componentStack ?? "no stack"})`
    ).catch(() => undefined)
  }

  render() {
    const { error } = this.state
    if (!error) return this.props.children
    return (
      <div className="mx-auto w-full max-w-2xl p-6">
        <div className="space-y-3 rounded-2xl border border-destructive/40 bg-destructive/10 p-5">
          <h1 className="font-medium text-destructive">Something broke</h1>
          <p className="text-sm text-foreground/80">{error.message}</p>
          <p className="text-xs text-muted-foreground">
            The full error was written to the app log. Reloading usually
            recovers; the work in progress is not lost.
          </p>
          <button
            type="button"
            onClick={() => window.location.reload()}
            className="rounded-lg border bg-card px-3 py-1.5 text-sm font-medium transition-colors hover:bg-muted"
          >
            Reload
          </button>
        </div>
      </div>
    )
  }
}
