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
    {/* Every motion component honours the OS "reduce motion" setting. The CSS
        animations have their own media query; this covers the JS-driven ones
        (preview spring, card rise, countdown roll, voice pulse). */}
    <MotionConfig reducedMotion="user">
      <ThemeProvider>
        <ExternalLinkGuard />
        {/*{import.meta.env.DEV ? <DebugPanel /> : null}*/}
        <main data-ui-scroll-container>
          <App />
        </main>
        <AppToaster />
      </ThemeProvider>
    </MotionConfig>
  </StrictMode>
)
