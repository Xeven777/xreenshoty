import { StrictMode } from "react"
import { createRoot } from "react-dom/client"

import { RegionOverlay } from "@/components/region-overlay"

// No index.css: the overlay is styled inline on purpose. Pulling the 77 KB
// Tailwind sheet into this second webview measurably delayed selection.
createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <RegionOverlay />
  </StrictMode>,
)