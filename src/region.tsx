import { StrictMode } from "react"
import { createRoot } from "react-dom/client"

import { RegionOverlay } from "@/components/region-overlay"

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <RegionOverlay />
  </StrictMode>
)
