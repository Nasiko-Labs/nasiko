// Stale-build recovery. Ported from codex/nasiko-react-foundation ui/shared/app-core/src/stale-build.ts.
//
// A tab opened before an upgrade keeps running the old build. Its HTML and the chunks it has already loaded keep
// working (hashed assets are immutable and cached), but a chunk it has not loaded yet no longer exists on the server:
// the new binary embeds only its own build and answers the old name with a non-cacheable 404 (oss/spa). The same
// happens for a moment during a rolling update, when the HTML and a chunk come from replicas on different versions.
//
// The rule: never lose unsaved work silently, never loop, and never mistake a network problem for a new version.
//
// A failed chunk is first diagnosed (`checkVersion`): the server's `index.html` (fetched `no-store`) names the entry
// script of the build it serves; if that differs from the entry this tab booted with, the build changed. Otherwise the
// failure is ordinary: the tab is offline, the server is unreachable, or the same build failed to serve a chunk.
//
// - A chunk that only *preloads* (hover/focus intent, `vite:preloadError`) marks the build stale only when the version
//   check says it changed; the shell shows a notice and nothing reloads, because the user may be in the middle of an
//   edit (app/shell/StaleBuildNotice.tsx).
// - A chunk error that reaches the route error boundary (app/shell/RouteStates.tsx) runs `recoverFromChunkError`:
//   - version changed, no unsaved edits (the leave guards report them here): reload once onto the same URL;
//   - version changed, unsaved edits: no reload ("held"); the page asks to save first;
//   - offline: no reload (it would show the browser's offline page); wait for `online`, then reload once;
//   - server unreachable, or the same build failed: no automatic reload; the user gets Try again.
// - At most MAX_RELOADS automatic reloads within RELOAD_WINDOW_MS (sessionStorage marker), so a flapping rollout ends
//   in a Reload button rather than a reload loop.
// - Before React mounts (the entry script itself is gone), BOOT_GUARD_SCRIPT does the same with plain DOM.
//
// ponytail: a route *component* chunk is loaded by TanStack's lazyRouteComponent, which reloads once per failed chunk
// itself (sessionStorage `tanstack_router_reload:<message>`) before this boundary sees the error, without the offline
// or unsaved checks. This module handles what that reload doesn't fix, loader chunks, preloads and the boot. Upgrade
// path if its blind reload matters: wrap route components in our own lazy loader that calls recoverFromChunkError.
import { useSyncExternalStore } from 'react'
import { fetchApi } from '@nasiko/data-access/client'
import { MAX_RELOADS, RELOAD_KEY, recentReloads } from '@nasiko/app-core/staleBuildBoot'

export { MAX_RELOADS, RELOAD_KEY, RELOAD_WINDOW_MS } from '@nasiko/app-core/staleBuildBoot'

/** Dispatched on `window` when the build is found to be stale (the notice listens; tests can too). */
export const STALE_BUILD_EVENT = 'nasiko:stale-build'

const CHUNK_ERROR =
  /Failed to fetch dynamically imported module|error loading dynamically imported module|Importing a module script failed|Unable to preload CSS|Outdated Optimize Dep/i

/** Is this the error a missing or unreachable code chunk produces (Chromium, Firefox, Safari, Vite's CSS preload)? */
export function isChunkLoadError(error: unknown): boolean {
  if (error instanceof Error)
    return CHUNK_ERROR.test(error.message) || isChunkLoadError(error.cause)
  return typeof error === 'string' && CHUNK_ERROR.test(error)
}

// ── Unsaved edits ───────────────────────────────────────────────────────────────────────────────────────────────────
// Only leave guards register. An upload or a running chat turn has its own beforeunload prompt, which an automatic
// reload still meets.
const unsaved = new Set<symbol>()

/** A leave guard with unsaved edits registers here while they exist; returns the unregister. */
export function registerUnsavedEdits(): () => void {
  const token = Symbol('unsaved')
  unsaved.add(token)
  return () => {
    unsaved.delete(token)
  }
}

export function hasUnsavedEdits(): boolean {
  return unsaved.size > 0
}

// ── Staleness ───────────────────────────────────────────────────────────────────────────────────────────────────────
let stale = false
let recovery: StaleRecovery | null = null
const listeners = new Set<() => void>()

function notify() {
  for (const listener of listeners) listener()
}

function markStale() {
  if (stale) return
  stale = true
  notify()
  window.dispatchEvent(new Event(STALE_BUILD_EVENT))
}

function subscribe(listener: () => void) {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/** True once this tab has seen a chunk it could not load from a build the server no longer serves. */
export function useStaleBuild(): boolean {
  return useSyncExternalStore(
    subscribe,
    () => stale,
    () => false,
  )
}

/** The outcome of the last route recovery in this tab (null: none attempted). */
export function useStaleRecovery(): StaleRecovery | null {
  return useSyncExternalStore(
    subscribe,
    () => recovery,
    () => null,
  )
}

/** The reloads recorded in the window, or null when storage is unavailable (a loop could not be ruled out). */
function readReloads(now: number): number[] | null {
  try {
    return recentReloads(window.sessionStorage.getItem(RELOAD_KEY), now)
  } catch {
    return null
  }
}

/**
 * What the recovery did: reloaded; held back for unsaved edits; gave up because a reload already failed; or, when the
 * build did not change: offline (waiting for the connection), network (server unreachable), failed (same build).
 */
export type StaleRecovery =
  'checking' | 'reloading' | 'held' | 'exhausted' | 'offline' | 'network' | 'failed'

/** What a failed chunk most likely means. */
export type VersionCheck = 'changed' | 'same' | 'unreachable' | 'offline'

const ENTRY = /<script\b[^>]*\btype="module"[^>]*\bsrc="([^"]+)"/

/** The entry script this tab booted with (the module script in the document it loaded). */
function bootEntry(): string | null {
  const script = document.querySelector<HTMLScriptElement>('script[type="module"][src]')
  return script ? new URL(script.src, location.href).pathname : null
}

type Fetch = (url: string, init: RequestInit) => Promise<Response>

/**
 * Ask the server which build it serves now. Never throws. The shell is read from the page's origin, not the API base
 * (the MT dashboard points the API at a workspace): `fetchApi` keeps an absolute URL as it is. `fetchImpl` and
 * `online` are injectable for tests.
 */
export async function checkVersion(
  fetchImpl: Fetch = fetchApi,
  online: () => boolean = () => navigator.onLine,
): Promise<VersionCheck> {
  if (!online()) return 'offline'
  const mine = bootEntry()
  try {
    const response = await fetchImpl(new URL('/', location.origin).href, {
      cache: 'no-store',
      headers: { accept: 'text/html' },
    })
    if (!response.ok) return 'unreachable'
    const match = ENTRY.exec(await response.text())
    if (!match?.[1] || !mine) return 'unreachable'
    return new URL(match[1], location.href).pathname === mine ? 'same' : 'changed'
  } catch {
    return online() ? 'unreachable' : 'offline'
  }
}

function setRecovery(next: StaleRecovery) {
  recovery = next
  notify()
}

let waitingForOnline = false

/**
 * A route could not load its code. Diagnose, then recover as the header describes. Resolves with the outcome (also
 * published to `useStaleRecovery`). Injectable for tests.
 */
export async function recoverFromChunkError(
  deps: {
    check?: () => Promise<VersionCheck>
    reload?: () => void
    now?: () => number
    target?: Window
  } = {},
): Promise<StaleRecovery> {
  const check = deps.check ?? (() => checkVersion())
  const reload =
    deps.reload ??
    (() => {
      window.location.reload()
    })
  const now = deps.now ?? (() => Date.now())
  if (recovery === 'reloading') return recovery
  setRecovery('checking')
  const verdict = await check()
  if (verdict === 'changed') return recoverFromStaleRoute(reload, now())
  if (verdict === 'same') {
    setRecovery('failed')
    return 'failed'
  }
  if (verdict === 'unreachable') {
    setRecovery('network')
    return 'network'
  }
  setRecovery('offline')
  const target = deps.target ?? window
  if (!waitingForOnline) {
    waitingForOnline = true
    target.addEventListener(
      'online',
      () => {
        waitingForOnline = false
        // The failed import is cached by React and the browser: only a reload retries it. Bounded as ever.
        setRecovery(decide(reload, now()))
      },
      { once: true },
    )
  }
  return 'offline'
}

/**
 * The build changed under a route that needs code. Reload onto the same URL once, unless that would lose unsaved
 * edits or a reload just failed to fix it. `reload` is injectable for tests.
 */
export function recoverFromStaleRoute(
  reload: () => void = () => {
    window.location.reload()
  },
  now: number = Date.now(),
): StaleRecovery {
  markStale()
  // Once a reload is under way, a second boundary (or StrictMode's double effect) must not report it as failed.
  if (recovery === 'reloading') return recovery
  recovery = decide(reload, now)
  notify()
  return recovery
}

function decide(reload: () => void, now: number): StaleRecovery {
  if (hasUnsavedEdits()) return 'held'
  const reloads = readReloads(now)
  if (!reloads || reloads.length >= MAX_RELOADS) return 'exhausted'
  try {
    window.sessionStorage.setItem(RELOAD_KEY, [...reloads, now].join(','))
  } catch {
    return 'exhausted'
  }
  reload()
  return 'reloading'
}

/**
 * Listen for Vite's preload failures (intent preloads and route chunks alike): when the version check says the build
 * changed, mark it stale. Never reloads. `check` is injectable for tests.
 */
export function installStaleBuildDetection(
  target: Window = window,
  check: () => Promise<VersionCheck> = () => checkVersion(),
): void {
  let pending = false
  target.addEventListener('vite:preloadError', () => {
    if (stale || pending) return
    pending = true
    void check().then((verdict) => {
      pending = false
      if (verdict === 'changed') markStale()
    })
  })
}

/** Test seam: forget the state of this module. */
export function resetStaleBuildForTests(): void {
  stale = false
  recovery = null
  waitingForOnline = false
  unsaved.clear()
}
