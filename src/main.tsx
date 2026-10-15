import { StrictMode } from "react"
import { createRoot } from "react-dom/client"
import { MotionConfig } from "motion/react"

import "./index.css"
import App from "./App.tsx"
import { ThemeProvider } from "@/components/theme-provider.tsx"
import { AppToaster } from "@/components/toaster.tsx"
import { ExternalLinkGuard } from "./components/external-link-guard.tsx"
// import { DebugPanel } from "./components/debug-panel.tsx"

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <MotionConfig reducedMotion="user">
      <ThemeProvider>
        <ExternalLinkGuard />
        {/*{import.meta.env.DEV ? <DebugPanel /> : null}*/}
        <main data-ui-scroll-container className="relative">
          <div className="absolute rotate-180 z-[-2] h-screen w-screen bg-transparent bg-[radial-gradient(ellipse_80%_80%_at_50%_-20%,rgba(240,111,200,0.2),rgba(255,255,255,0))]"></div>
          <App />
        </main>
        <AppToaster />
      </ThemeProvider>
    </MotionConfig>
  </StrictMode>
)
