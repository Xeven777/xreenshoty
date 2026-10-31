import { StrictMode } from "react"
import { createRoot } from "react-dom/client"
import { MotionConfig } from "motion/react"
import { error as logError } from "@tauri-apps/plugin-log"

import "./index.css"
import App from "./App.tsx"
import { ErrorBoundary } from "@/components/error-boundary.tsx"
import { ThemeProvider } from "@/components/theme-provider.tsx"
import { AppToaster } from "@/components/toaster.tsx"
import { ExternalLinkGuard } from "./components/external-link-guard.tsx"
// import { DebugPanel } from "./components/debug-panel.tsx"

// A webview exception unmounts the whole React tree, and since `body` carries
// the app background that presents as a black window with a live process and no
// Rust log entry — indistinguishable from a hang. Forwarding JS errors into the
// same log file is what turns "it went black" into a diagnosable line.
window.addEventListener("error", (e) => {
  void logError(`webview: ${e.message} @ ${e.filename}:${e.lineno}`).catch(
    () => undefined
  )
})
window.addEventListener("unhandledrejection", (e) => {
  void logError(`webview unhandled rejection: ${String(e.reason)}`).catch(
    () => undefined
  )
})

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <MotionConfig reducedMotion="user">
      <ThemeProvider>
        <ExternalLinkGuard />
        {/*{import.meta.env.DEV ? <DebugPanel /> : null}*/}
        <ErrorBoundary>
          <main data-ui-scroll-container className="relative">
            <div className="absolute rotate-180 z-[-2] h-screen w-screen bg-transparent bg-[radial-gradient(ellipse_80%_80%_at_50%_-20%,rgba(240,111,200,0.2),rgba(255,255,255,0))]"></div>
            <App />
          </main>
        </ErrorBoundary>
        <AppToaster />
      </ThemeProvider>
    </MotionConfig>
  </StrictMode>
)
