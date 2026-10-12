import { Toaster as SonnerToaster } from "sonner"
import { useTheme } from "./theme-provider"

export function AppToaster() {
  const { theme } = useTheme()
  return (
    <SonnerToaster
      richColors
      closeButton
      position="bottom-center"
      theme={theme}
    />
  )
}
