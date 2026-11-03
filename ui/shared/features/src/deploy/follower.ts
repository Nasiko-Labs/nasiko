/**
 * The background build follower (plans/feat-deploy.md §5 Background follow; design review 8; eng review R8, R3).
 *
 * Builds started in this tab and session (an upload, a GitHub clone) are followed outside React, like chat's turn
 * registry. There are no background streams (R8): while anything is followed and the tab is visible, one poll of
 * `GET /api/agents/uploads?limit=20` every `FOLLOW_POLL_MS` covers every followed build (the upload id is the build id).
 * The open Build page has its own stream, so a build finishing there is dropped without a toast. Anywhere else, a finish
 * calls the notifier once: "<name> is running · Chat with it" or "<name> failed · See why".
 *
 * A registry import is synchronous (D-7): it arrives here already finished and is announced unless the Registry tab
 * that started it is still open (it navigates on its own).
 *
 * After a reload nothing is followed (Builds is the place to check). Sign-out and another account's sign-in drop
 * everything (`clearFollower`, from `clearUploads` in `stopDeployWork`, R3).
 */
import type { QueryClient } from '@tanstack/react-query'
import { recentUploadsQuery } from './api'
import {
  FOLLOW_BACKOFF_MAX_MS,
  FOLLOW_MAX_MS,
  FOLLOW_POLL_MS,
  FOLLOW_UPLOADS_LIMIT,
} from './tuning'
import type { UploadStatus } from './types'

export type FollowNotice =
  | { kind: 'running'; buildId: string | null; agentId: string | null; name: string }
  | { kind: 'failed'; buildId: string; name: string }
  /** A registry import whose deploy didn't start (`container_name: null`, D-7). */
  | { kind: 'notRunning'; agentId: string; name: string }

interface Followed {
  name: string
  /** Known for uploads (the 202 body); a GitHub clone's answer has none, so Chat falls back to the name. */
  agentId: string | null
  since: number
}

type FetchUploads = () => Promise<UploadStatus[]>
type Notify = (n: FollowNotice) => void

const followed = new Map<string, Followed>()
const listeners = new Set<() => void>()
let fetchUploads: FetchUploads | null = null
let client: QueryClient | null = null
let notify: Notify = () => {}
let openBuild: string | null = null
let registryOpen = 0
let timer: ReturnType<typeof setTimeout> | null = null
let polling = false
/** Consecutive failed reads: the next poll backs off (a stopped server isn't hammered every 5 s). */
let failures = 0
let now: () => number = () => Date.now()

function emit() {
  for (const l of listeners) l()
}

export function subscribeFollower(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** Builds still followed: the Builds nav item's count. */
export const followedCount = () => followed.size

/** The app shell's toaster (one `Toaster`, eng review) registers here. Returns an unregister function. */
export function setNotifier(fn: Notify): () => void {
  notify = fn
  return () => {
    if (notify === fn) notify = () => {}
  }
}

/** Tests: a fixed clock. */
export function setFollowerClock(fn: () => number) {
  now = fn
}

/** The Build page tells the follower which build it streams itself (R8). */
export function setOpenBuild(id: string | null) {
  openBuild = id
}

/** The Registry tab is open: it shows an import's outcome itself. Returns the close function. */
export function openRegistryView(): () => void {
  registryOpen += 1
  return () => {
    registryOpen = Math.max(0, registryOpen - 1)
  }
}

/** Follow a build started in this session. `qc` is the app QueryClient, so the poll takes the app's 401 path. */
export function follow(
  qc: QueryClient,
  buildId: string,
  name: string,
  agentId: string | null = null,
) {
  client = qc
  fetchUploads ??= () =>
    qc.fetchQuery({ ...recentUploadsQuery(FOLLOW_UPLOADS_LIMIT), staleTime: 0 })
  if (followed.has(buildId)) return
  // Nothing was being followed: an old failure streak doesn't delay this build's first read.
  if (followed.size === 0) failures = 0
  followed.set(buildId, { name, agentId, since: now() })
  emit()
  schedule()
}

/** A registry import finished (it is synchronous, so there is nothing to poll). */
/** Returns true when it toasted (the Registry tab wasn't open to show it). */
export function importFinished(n: FollowNotice): boolean {
  if (registryOpen > 0) return false
  notify(n)
  return true
}

/** Tests: replace the uploads read. */
export function setUploadsFetcher(fn: FetchUploads | null) {
  fetchUploads = fn
}

export function clearFollower() {
  followed.clear()
  if (timer) clearTimeout(timer)
  timer = null
  openBuild = null
  failures = 0
  fetchUploads = null
  client = null
  emit()
}

const visible = () => typeof document === 'undefined' || document.visibilityState !== 'hidden'

function schedule() {
  if (timer || polling || followed.size === 0 || !visible()) return
  timer = setTimeout(
    () => {
      timer = null
      void pollOnce().finally(schedule)
    },
    Math.min(FOLLOW_POLL_MS * 2 ** failures, FOLLOW_BACKOFF_MAX_MS),
  )
}

if (typeof document !== 'undefined') {
  document.addEventListener('visibilitychange', () => {
    if (visible()) schedule()
    else if (timer) {
      clearTimeout(timer)
      timer = null
    }
  })
}

/** One poll for every followed build. Exported for tests; the timer calls it. */
export async function pollOnce(): Promise<void> {
  if (!followed.size || !fetchUploads || polling) return
  polling = true
  let rows: UploadStatus[] | null = null
  try {
    rows = await fetchUploads()
    failures = 0
  } catch {
    // Retried with backoff; a 401 has already taken the app's expired() path.
    failures += 1
  } finally {
    polling = false
  }
  const byId = new Map((rows ?? []).map((r) => [r.upload_id, r]))
  let changed = false
  let finished = false
  for (const [id, f] of [...followed]) {
    const row = byId.get(id)
    const done = row?.status === 'completed' || row?.status === 'failed'
    // Older than any build can run (the worker times out at 30 min) and still not seen finishing, or unreadable that
    // long: stop following, silently.
    if (!done && now() - f.since < FOLLOW_MAX_MS) continue
    followed.delete(id)
    changed = true
    if (!done || !row) continue
    finished = true
    if (id === openBuild) continue
    const name = row.agent_name || f.name
    notify(
      row.status === 'completed'
        ? { kind: 'running', buildId: id, agentId: f.agentId, name }
        : { kind: 'failed', buildId: id, name },
    )
  }
  // A finished build changes its agent (status, version, versions): the agent lists and that agent's pages re-read.
  // The restart watches (`['agents','watch',id]`) are cache-only state and stay.
  if (finished)
    void client?.invalidateQueries({
      queryKey: ['agents'],
      predicate: (q) => q.queryKey[1] !== 'watch',
    })
  if (changed) emit()
}
