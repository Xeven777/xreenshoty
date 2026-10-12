import { useState } from "react"
import { Camera, Gear, Keyboard, MagnifyingGlass } from "@phosphor-icons/react"

import { Capture } from "@/components/capture"
import { IndexIndicator } from "@/components/index-indicator"
import { Search } from "@/components/search"
import { SettingsPanel } from "@/components/settings-panel"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { useSettings } from "@/lib/settings"

export default function App() {
  const { settings, update } = useSettings()
  const [shortcutError, setShortcutError] = useState<string | null>(null)

  return (
    <div className="mx-auto w-full max-w-5xl space-y-6 p-6">
      <header className="flex items-center gap-4">
        <img
          src="/logo.svg"
          alt="xreenshoty logo"
          className="size-12 shrink-0 rounded-md shadow-sm transition-transform duration-300 hover:scale-102 active:scale-95 active:rotate-2"
        />
        <div className="min-w-0 flex-1">
          <h1 className="text-2xl font-semibold tracking-tight">xreenshoty</h1>
          <p className="truncate text-sm text-muted-foreground">
            Local-first screenshots you can search later
          </p>
        </div>
      </header>

      <div className="flex justify-end">
        <IndexIndicator />
      </div>

      <Tabs defaultValue="capture" className="gap-4">
        <TabsList>
          <TabsTrigger value="capture">
            <Camera data-icon="inline-start" />
            Capture
          </TabsTrigger>
          <TabsTrigger value="search">
            <MagnifyingGlass data-icon="inline-start" />
            Search
          </TabsTrigger>
          <TabsTrigger value="settings">
            <Gear data-icon="inline-start" />
            Settings
          </TabsTrigger>
        </TabsList>

        <TabsContent value="capture" keepMounted>
          <Capture
            settings={settings}
            update={update}
            onShortcutError={setShortcutError}
          />
        </TabsContent>

        {/* keepMounted for the same reason as Capture: the thumbnail cache and
            results survive a tab round-trip. */}
        <TabsContent value="search" keepMounted>
          <Search />
        </TabsContent>

        <TabsContent value="settings">
          <SettingsPanel
            settings={settings}
            update={update}
            shortcutError={shortcutError}
          />
        </TabsContent>
      </Tabs>

      <footer className="flex items-center justify-center gap-2 pb-2 text-center text-xs text-muted-foreground">
        <Keyboard size={14} />
        Fully offline · files stay in your Pictures folder
      </footer>
    </div>
  )
}
