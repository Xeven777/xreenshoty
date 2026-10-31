import { useCallback, useEffect, useMemo, useState } from "react"
import { isTauri } from "@tauri-apps/api/core"
import {
  ArrowsClockwise,
  CaretLeft,
  MagnifyingGlass,
  SpinnerGap,
  Trash,
  UsersThree,
  X,
} from "@phosphor-icons/react"
import { motion } from "motion/react"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { Input } from "@/components/ui/input"
import { ShotCard, ShotPreview, usePreview } from "@/components/shot"
import {
  clusterScreenshots,
  faceClusters,
  faceStatus,
  forgetCluster,
  mergeClusters,
  openFolderSafe,
  reclusterFaces,
  renameCluster,
  searchByFace,
  type FaceCluster,
  type FaceStatus,
  type Screenshot,
  type SearchHit,
} from "@/lib/screenshot"
import {
  errorText,
  toastFailed,
  toastForgotten,
  toastReclustered,
} from "@/lib/toast"
import { cn, dirOf, plural } from "@/lib/utils"
import { useFaceThumbnails } from "@/lib/use-face-thumbnails"
import { useThumbnails } from "@/lib/use-thumbnails"

/** Auto-ids are never ground truth: a user name wins, otherwise a placeholder. */
function personName(cluster: FaceCluster, index: number) {
  return cluster.label?.trim() || `Person ${index + 1}`
}

/** Face crop, or a placeholder. Shared by the person card and the detail
 *  header so the two cannot drift apart. */
function FaceThumb({
  name,
  thumb,
  className,
}: {
  name: string
  thumb?: string
  className?: string
}) {
  if (!thumb) {
    return (
      <div className="grid size-full animate-pulse place-items-center">
        <UsersThree size={20} className="text-muted-foreground/50" />
      </div>
    )
  }
  return (
    <img
      src={thumb}
      alt={name}
      decoding="async"
      className={cn("size-full object-cover", className)}
    />
  )
}

function PersonCard({
  cluster,
  index,
  thumb,
  onOpen,
}: {
  cluster: FaceCluster
  index: number
  thumb?: string
  onOpen: (cluster: FaceCluster) => void
}) {
  const name = personName(cluster, index)
  const shots = cluster.screenshot_ids.length
  return (
    <motion.button
      type="button"
      onClick={() => onOpen(cluster)}
      title={`View ${name}`}
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.2, delay: Math.min(index, 8) * 0.03 }}
      className="group overflow-hidden rounded-xl border bg-card text-left transition-[transform,box-shadow,border-color] duration-150 outline-none focus-visible:border-primary/40 hover:-translate-y-0.5 hover:border-primary/40 hover:shadow-lg"
    >
      <div className="relative aspect-square overflow-hidden bg-muted">
        <FaceThumb
          name={name}
          thumb={thumb}
          className="transition-transform duration-200 group-hover:scale-[1.03]"
        />
      </div>
      <div className="space-y-0.5 p-2.5">
        <p className="truncate text-sm font-medium">{name}</p>
        <p className="text-[11px] text-muted-foreground">
          {plural(cluster.face_count, "face")} · {plural(shots, "screenshot")}
        </p>
      </div>
    </motion.button>
  )
}

/** 7.6: KNN over SFace vectors from one reference face. */
function PersonView({
  clusters,
  cluster,
  onBack,
  onChanged,
}: {
  clusters: FaceCluster[]
  cluster: FaceCluster
  onBack: () => void
  onChanged: () => void
}) {
  const preview = usePreview()
  const [shots, setShots] = useState<Screenshot[] | null>(null)
  const [similar, setSimilar] = useState<SearchHit[] | null>(null)
  const [finding, setFinding] = useState(false)
  const [failure, setFailure] = useState<string | null>(null)
  const [name, setName] = useState(cluster.label ?? "")
  const [saving, setSaving] = useState(false)
  const [merging, setMerging] = useState(false)
  const [forgetting, setForgetting] = useState(false)
  const [confirmForget, setConfirmForget] = useState(false)

  const index = clusters.findIndex((c) => c.cluster_id === cluster.cluster_id)
  const others = clusters.filter((c) => c.cluster_id !== cluster.cluster_id)

  const faceThumbs = useFaceThumbnails([cluster.representative_face_id])

  // Remounted per cluster (keyed by the parent), so `shots`/`similar` start
  // fresh for the person on screen without a synchronous reset here.
  useEffect(() => {
    let cancelled = false
    clusterScreenshots(cluster.cluster_id)
      .then((s) => {
        if (!cancelled) setShots(s)
      })
      .catch((e) => {
        if (!cancelled) {
          setShots([])
          setFailure(errorText(e))
        }
      })
    return () => {
      cancelled = true
    }
  }, [cluster.cluster_id])

  // Two-step delete: one stray click must not drop a person, and the armed
  // state has to expire on its own or the button silently stays live.
  useEffect(() => {
    if (!confirmForget) return
    const timer = setTimeout(() => setConfirmForget(false), 4000)
    return () => clearTimeout(timer)
  }, [confirmForget])

  const shown = useMemo(() => similar ?? shots ?? [], [similar, shots])
  const thumbs = useThumbnails(useMemo(() => shown.map((s) => s.path), [shown]))
  const scoreByPath = useMemo(
    () => new Map((similar ?? []).map((h) => [h.path, h])),
    [similar]
  )

  const openFolderOf = useCallback((item: Screenshot) => {
    void openFolderSafe(dirOf(item.path))
  }, [])

  async function saveName() {
    const trimmed = name.trim()
    if (trimmed === (cluster.label ?? "")) return
    setSaving(true)
    try {
      await renameCluster(cluster.cluster_id, trimmed || null)
      onChanged()
    } catch (e) {
      toastFailed("Couldn't rename", e)
    } finally {
      setSaving(false)
    }
  }

  async function doMerge(target: FaceCluster) {
    setMerging(true)
    try {
      await mergeClusters(cluster.cluster_id, target.cluster_id)
      onChanged()
      onBack()
    } catch (e) {
      toastFailed("Couldn't merge", e)
    } finally {
      setMerging(false)
    }
  }

  async function doForget() {
    if (!confirmForget) {
      setConfirmForget(true)
      return
    }
    setForgetting(true)
    try {
      const removed = await forgetCluster(cluster.cluster_id)
      toastForgotten(removed)
      onChanged()
      onBack()
    } catch (e) {
      toastFailed("Couldn't remove person", e)
    } finally {
      setForgetting(false)
    }
  }

  async function findSimilar() {
    setFinding(true)
    setFailure(null)
    try {
      setSimilar(await searchByFace(cluster.representative_face_id))
    } catch (e) {
      setFailure(errorText(e))
      toastFailed("Find similar failed", e)
    } finally {
      setFinding(false)
    }
  }

  const name0 = personName(cluster, index)
  const avatar = faceThumbs[cluster.representative_face_id]

  return (
    <div className="space-y-4">
      <section className="flex flex-wrap items-start gap-4 rounded-2xl border bg-card p-5 shadow-sm">
        <Button
          size="icon-sm"
          variant="ghost"
          title="Back to people"
          onClick={onBack}
        >
          <CaretLeft />
        </Button>

        <div className="size-16 shrink-0 overflow-hidden rounded-xl bg-muted">
          <FaceThumb name={name0} thumb={avatar} />
        </div>

        <div className="min-w-0 flex-1 space-y-1">
          <div className="flex items-center gap-1.5">
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault()
                  e.currentTarget.blur()
                } else if (e.key === "Escape") {
                  setName(cluster.label ?? "")
                  e.currentTarget.blur()
                }
              }}
              onBlur={() => void saveName()}
              placeholder={name0}
              aria-label="Person name"
              className="h-8 max-w-60"
            />
            {saving ? (
              <SpinnerGap size={14} className="animate-spin text-muted-foreground" />
            ) : null}
          </div>
          <p className="text-xs text-muted-foreground">
            {plural(cluster.face_count, "face")} across{" "}
            {plural(cluster.screenshot_ids.length, "screenshot")}
          </p>
        </div>

        <div className="flex shrink-0 items-center gap-1.5">
          {similar ? (
            <Button
              size="sm"
              variant="secondary"
              onClick={() => setSimilar(null)}
            >
              <X data-icon="inline-start" />
              Show group only
            </Button>
          ) : (
            <Button
              size="sm"
              variant="secondary"
              disabled={finding}
              onClick={() => void findSimilar()}
              title="Find screenshots with similar faces, beyond this group"
            >
              {finding ? (
                <SpinnerGap data-icon="inline-start" className="animate-spin" />
              ) : (
                <MagnifyingGlass data-icon="inline-start" />
              )}
              Find similar
            </Button>
          )}

          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button
                  size="sm"
                  variant="secondary"
                  disabled={merging || others.length === 0}
                  title="Merge this person into another"
                >
                  Merge
                </Button>
              }
            />
            <DropdownMenuContent align="end" className="w-64">
              {/* The label is a `Menu.GroupLabel`, which Base UI only accepts
                  inside a Group — used bare it throws during render. */}
              <DropdownMenuGroup>
                <DropdownMenuLabel>Merge into…</DropdownMenuLabel>
                {others.map((other) => (
                  <DropdownMenuItem
                    key={other.cluster_id}
                    onClick={() => void doMerge(other)}
                  >
                    {personName(
                      other,
                      clusters.findIndex(
                        (c) => c.cluster_id === other.cluster_id
                      )
                    )}
                    <span className="ml-auto text-xs text-muted-foreground">
                      {other.face_count}
                    </span>
                  </DropdownMenuItem>
                ))}
              </DropdownMenuGroup>
            </DropdownMenuContent>
          </DropdownMenu>

          <Button
            size="sm"
            variant="destructive"
            disabled={forgetting}
            onClick={() => void doForget()}
            title={
              confirmForget
                ? `Remove ${name0} from People — the screenshots stay`
                : `Remove ${name0} from People`
            }
          >
            {forgetting ? (
              <SpinnerGap data-icon="inline-start" className="animate-spin" />
            ) : (
              <Trash data-icon="inline-start" />
            )}
            {confirmForget ? "Confirm" : "Remove"}
          </Button>
        </div>
      </section>

      {failure ? (
        <p className="rounded-xl bg-destructive/10 p-3 text-sm text-destructive">
          {failure}
        </p>
      ) : null}

      {similar ? (
        <div className="flex items-center gap-2">
          <Badge variant="secondary">{similar.length}</Badge>
          <span className="text-xs text-muted-foreground">
            closest faces by similarity — may include people not yet grouped
          </span>
        </div>
      ) : null}

      {shots === null && !similar ? (
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
          {Array.from({ length: 6 }).map((_, i) => (
            <div key={i} className="overflow-hidden rounded-xl border bg-card">
              <div className="aspect-video animate-pulse bg-muted" />
              <div className="space-y-1.5 p-2.5">
                <div className="h-3 w-2/3 animate-pulse rounded bg-muted" />
                <div className="h-2.5 w-1/2 animate-pulse rounded bg-muted" />
              </div>
            </div>
          ))}
        </div>
      ) : shown.length === 0 ? (
        <p className="rounded-xl border bg-card p-6 text-sm text-muted-foreground">
          No screenshots for this person.
        </p>
      ) : (
        <div className="shot-grid grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
          {shown.map((item, i) => (
            <ShotCard
              key={item.path}
              item={item}
              index={i}
              thumb={thumbs[item.path]}
              score={scoreByPath.get(item.path)?.score ?? null}
              scoreZ={scoreByPath.get(item.path)?.score_z ?? null}
              onOpen={preview.show}
              onOpenFolder={openFolderOf}
            />
          ))}
        </div>
      )}

      <ShotPreview
        item={preview.item}
        score={
          preview.item
            ? (scoreByPath.get(preview.item.path)?.score ?? null)
            : null
        }
        scoreZ={
          preview.item
            ? (scoreByPath.get(preview.item.path)?.score_z ?? null)
            : null
        }
        onClose={preview.hide}
        onOpenFolder={openFolderOf}
      />
    </div>
  )
}

export function People({
  faceIndexing,
  active,
}: {
  faceIndexing: boolean
  active: boolean
}) {
  const tauri = isTauri()
  const [clusters, setClusters] = useState<FaceCluster[] | null>(null)
  const [status, setStatus] = useState<FaceStatus | null>(null)
  const [failure, setFailure] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [selectedId, setSelectedId] = useState<number | null>(null)
  const [filter, setFilter] = useState("")

  const refresh = useCallback(async () => {
    try {
      const [list, stats] = await Promise.all([faceClusters(), faceStatus()])
      setClusters(list)
      setStatus(stats)
      setFailure(null)
      return list
    } catch (e) {
      setFailure(errorText(e))
      setClusters([])
      return [] as FaceCluster[]
    }
  }, [])

  // Refetch every time the tab is opened: faces are grouped in the background
  // by the indexer, so a list fetched once would go stale silently.
  useEffect(() => {
    if (!tauri || !active || !faceIndexing) return
    let cancelled = false
    faceClusters()
      .then((list) => {
        if (!cancelled) {
          setClusters(list)
          setFailure(null)
        }
      })
      .catch((e) => {
        if (!cancelled) {
          setFailure(errorText(e))
          setClusters([])
        }
      })
    return () => {
      cancelled = true
    }
  }, [tauri, active, faceIndexing])

  const repIds = useMemo(
    () => (clusters ?? []).map((c) => c.representative_face_id),
    [clusters]
  )
  const thumbs = useFaceThumbnails(repIds)

  const selected = clusters?.find((c) => c.cluster_id === selectedId) ?? null

  // `index` is the *unfiltered* position, so filtering never renumbers
  // "Person 3" into "Person 1".
  const people = useMemo(() => {
    const all = (clusters ?? []).map((cluster, index) => ({ cluster, index }))
    const q = filter.trim().toLowerCase()
    if (!q) return all
    return all.filter(({ cluster, index }) =>
      personName(cluster, index).toLowerCase().includes(q)
    )
  }, [clusters, filter])

  // Screenshots whose face pass has not run yet. Clustering is instant, so the
  // only honest "work in progress" to show is the detection pass behind it.
  const pending = status
    ? Math.max(0, status.screenshots_total - status.screenshots_indexed)
    : 0
  const indexedPct = status && status.screenshots_total > 0
    ? Math.round(
        (status.screenshots_indexed / status.screenshots_total) * 100
      )
    : 0

  // Poll while detection is still outstanding so the bar actually moves. Only
  // while the tab is open — a hidden tab has nothing to show.
  useEffect(() => {
    if (!tauri || !active || !faceIndexing || pending <= 0) return
    const id = setInterval(() => void refresh(), 2000)
    return () => clearInterval(id)
  }, [tauri, active, faceIndexing, pending, refresh])

  async function runRecluster() {
    setBusy(true)
    try {
      const written = await reclusterFaces()
      const list = await refresh()
      toastReclustered(written, list.length)
    } catch (e) {
      toastFailed("Couldn't regroup people", e, () => void runRecluster())
    } finally {
      setBusy(false)
    }
  }

  if (!tauri) {
    return (
      <EmptyState
        title="People needs the desktop app"
        body="Face grouping runs locally in the Rust backend and is unavailable in the browser."
      />
    )
  }

  if (!faceIndexing) {
    return (
      <EmptyState
        title="Face indexing is off"
        body="Turn on Face indexing in Settings to detect and group faces. Everything stays on this machine, and off is the default."
      />
    )
  }

  if (selected) {
    return (
      <PersonView
        key={selected.cluster_id}
        clusters={clusters ?? []}
        cluster={selected}
        onBack={() => setSelectedId(null)}
        onChanged={() => void refresh()}
      />
    )
  }

  return (
    <div className="space-y-4">
      <section className="flex flex-wrap items-center justify-between gap-3 rounded-2xl border bg-card p-5 shadow-sm">
        <div className="min-w-0">
          <h2 className="font-medium">People</h2>
          <p className="text-xs text-muted-foreground">
            Faces detected on this machine, grouped by similarity. Names are
            yours to set; the grouping is only a suggestion.
          </p>
        </div>
        <Button
          size="sm"
          variant="secondary"
          disabled={busy}
          onClick={() => void runRecluster()}
          title="Regroup the faces already detected. Face detection itself keeps running in the background — new faces are grouped automatically as indexing reaches them."
        >
          {busy ? (
            <SpinnerGap data-icon="inline-start" className="animate-spin" />
          ) : (
            <ArrowsClockwise data-icon="inline-start" />
          )}
          Re-cluster
        </Button>
      </section>

      {status ? (
        <section className="space-y-2 rounded-2xl border bg-card p-4 shadow-sm">
          <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
            <span className="text-foreground">
              {status.cluster_count === 0
                ? "No people grouped yet"
                : `${plural(status.cluster_count, "person", "people")} · ${
                    status.clustered_faces
                  } of ${status.face_count} faces grouped`}
            </span>
            <span className="tabular-nums text-muted-foreground">
              {status.screenshots_indexed}/{status.screenshots_total}{" "}
              screenshots checked
            </span>
          </div>
          {status.screenshots_total > 0 ? (
            <div
              className="h-1.5 overflow-hidden rounded-full bg-muted"
              role="progressbar"
              aria-label="Face detection progress"
              aria-valuenow={indexedPct}
              aria-valuemin={0}
              aria-valuemax={100}
            >
              <div
                className={`h-full rounded-full transition-[width] duration-500 ease-out ${
                  pending > 0 ? "bg-brand" : "bg-emerald-500"
                }`}
                style={{ width: `${indexedPct}%` }}
              />
            </div>
          ) : null}
          <p className="text-[11px] text-muted-foreground">
            {pending > 0
              ? `Still detecting faces in ${plural(
                  pending,
                  "screenshot"
                )}. This runs in the background — nothing to wait for.`
              : status.face_count > 0
                ? `${plural(
                    status.face_count - status.clustered_faces,
                    "face"
                  )} seen once or too small to group, so they are left out on purpose.`
                : "No faces detected yet — most screenshots have none."}
          </p>
        </section>
      ) : null}

      {failure ? (
        <p className="rounded-xl bg-destructive/10 p-3 text-sm text-destructive">
          {failure}
        </p>
      ) : null}

      {clusters === null ? (
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
          {Array.from({ length: 8 }).map((_, i) => (
            <div
              key={i}
              className="overflow-hidden rounded-xl border bg-card"
            >
              <div className="aspect-square animate-pulse bg-muted" />
              <div className="space-y-1.5 p-2.5">
                <div className="h-3 w-2/3 animate-pulse rounded bg-muted" />
                <div className="h-2.5 w-1/2 animate-pulse rounded bg-muted" />
              </div>
            </div>
          ))}
        </div>
      ) : clusters.length === 0 ? (
        <EmptyState
          title="No people grouped yet"
          body={
            pending > 0
              ? "Faces are still being detected — the progress bar above shows how far along it is. A person needs two similar faces before they are grouped."
              : status && status.face_count > 0
                ? `All ${status.face_count} faces have only been seen once, so nothing is grouped yet. A second sighting of the same person is what creates a person here.`
                : "No faces were detected. Most screenshots simply do not contain one — try capturing something with a face in it."
          }
        />
      ) : (
        <div className="space-y-3">
          {clusters.length > 6 ? (
            <div className="relative max-w-sm">
              <MagnifyingGlass
                size={14}
                className="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-muted-foreground"
              />
              <Input
                value={filter}
                onChange={(e) => setFilter(e.target.value)}
                placeholder="Filter people by name"
                aria-label="Filter people by name"
                className="h-8 pl-8"
              />
            </div>
          ) : null}
          {people.length === 0 ? (
            <p className="rounded-xl border bg-card p-6 text-sm text-muted-foreground">
              No one here is named “{filter.trim()}”.
            </p>
          ) : (
            <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
              {people.map(({ cluster, index }) => (
                <PersonCard
                  key={cluster.cluster_id}
                  cluster={cluster}
                  index={index}
                  thumb={thumbs[cluster.representative_face_id]}
                  onOpen={(c) => setSelectedId(c.cluster_id)}
                />
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  )
}

function EmptyState({ title, body }: { title: string; body: string }) {
  return (
    <div className="flex items-center gap-3 rounded-xl border bg-card p-6 text-sm text-muted-foreground">
      <UsersThree size={20} className="shrink-0" />
      <div>
        <p className="font-medium text-foreground">{title}</p>
        <p className="text-xs">{body}</p>
      </div>
    </div>
  )
}
