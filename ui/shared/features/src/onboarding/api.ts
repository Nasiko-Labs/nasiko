/**
 * The onboarding read and write (nasiko-cloud-rs 41f776ae `onboarding.rs`). Small and import-light: the sidebar and the
 * Overview route read it from the shell chunk, while the dialog itself is lazy (index.ts).
 */
import { queryOptions, useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useSyncExternalStore } from 'react'
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { meQuery } from '@nasiko/data-access/auth'
import { apiFetch } from '@nasiko/data-access/client'
import { guideDue, skipKey, type StepId } from './logic'
import { onboardingSchema, type Onboarding, type Persona } from './types'

const PATH = '/api/me/onboarding'
const onboardingKeys = { all: ['onboarding'] as const }

const onboardingQuery = queryOptions({
  queryKey: onboardingKeys.all,
  queryFn: ({ signal }) => apiFetch<Onboarding>(PATH, { signal, schema: onboardingSchema }),
  // A probe: an older server answers a bare 404, which must settle at once (the page falls back to today's Overview).
  retry: false,
  // Only this user's own PATCH changes it, and that writes the cache.
  staleTime: Infinity,
})

const useOnboarding = () => useQuery(onboardingQuery)

/** The viewer's persona for defaults (the home's scope): settled once the read answers or fails (an older server's
 * bare 404 settles as no persona). */
export function usePersona(): { persona: Persona | null; settled: boolean } {
  const q = useOnboarding()
  return { persona: q.data?.persona ?? null, settled: !q.isPending }
}

/** Saves the persona; the server's first PATCH also completes onboarding. */
export function useSavePersona() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (persona: Persona) =>
      apiFetch<Onboarding>(PATH, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ persona }),
        schema: onboardingSchema,
      }),
    onSuccess: (data) => qc.setQueryData(onboardingKeys.all, data),
  })
}

// ─── "Skip guide" for this session ──────────────────────────────────────────────────────────────────────────────────
// localStorage under the drafts prefix: every tab agrees, and sign-out's draft clearing ends the skip (spec §2).

const listeners = new Set<() => void>()
function subscribe(fn: () => void) {
  listeners.add(fn)
  globalThis.addEventListener?.('storage', fn)
  return () => {
    listeners.delete(fn)
    globalThis.removeEventListener?.('storage', fn)
  }
}
function readSkipped(sub: string | undefined): boolean {
  if (!sub) return false
  try {
    return localStorage.getItem(skipKey(sub)) === '1'
  } catch {
    return false
  }
}
function writeSkipped(sub: string) {
  try {
    localStorage.setItem(skipKey(sub), '1')
  } catch {
    // Storage blocked: the skip lasts while this page is open.
    memorySkips.add(sub)
  }
  for (const fn of listeners) fn()
}
/** Storage blocked: remember the skip for this page load at least. */
const memorySkips = new Set<string>()

/** Whether the guide takes over `/` now (first-time and never opened by itself before), and how to record that it was. */
export function useGuide() {
  const me = useQuery(meQuery).data
  const onboarding = useOnboarding()
  const sub = me?.sub
  const skipped = useSyncExternalStore(
    subscribe,
    () => readSkipped(sub) || (!!sub && memorySkips.has(sub)),
    () => false,
  )
  const skip = useCallback(() => {
    if (sub) writeSkipped(sub)
  }, [sub])
  return {
    due: guideDue(onboarding.data, skipped),
    /** The server answered. */
    available: onboarding.isSuccess,
    /** An older server's bare 404 or another failure: the Overview behaves as before the guide. */
    absent: onboarding.isError,
    persona: onboarding.data?.persona ?? null,
    skip,
  }
}

// ─── The open guide ─────────────────────────────────────────────────────────────────────────────────────────────────
// One guide per page, opened by `/` for a first-time user or by the home's Setup guide; the sidebar hides Home
// while a first-run guide is open, so the shell and the route read the same state.

export interface GuideState {
  open: boolean
  /** Opened for a first-time user: the Overview stays hidden behind it. */
  firstRun: boolean
  step: StepId
}
let guideState: GuideState = { open: false, firstRun: false, step: 'welcome' }
const guideListeners = new Set<() => void>()
function setGuide(next: GuideState) {
  guideState = next
  for (const fn of guideListeners) fn()
}
const subscribeGuide = (fn: () => void) => {
  guideListeners.add(fn)
  return () => void guideListeners.delete(fn)
}
export const openGuide = (step: StepId, firstRun = false) =>
  setGuide({ open: true, firstRun, step })
export const closeGuide = () => {
  if (guideState.open) setGuide({ ...guideState, open: false, firstRun: false })
}
export const useGuideState = () =>
  useSyncExternalStore(
    subscribeGuide,
    () => guideState,
    () => guideState,
  )

/** The sidebar paths to hide: Home, while the guide stands in for it. */
export function useGuideHiddenNav(): readonly string[] {
  const { due } = useGuide()
  const state = useGuideState()
  return due || (state.open && state.firstRun) ? HIDE_HOME : NONE
}
const HIDE_HOME = ['/'] as const
const NONE = [] as const

// ─── The guide's prebuilt deploys ───────────────────────────────────────────────────────────────────────────────────
// Each import runs for minutes (seededDeploy.ts): the guide and Home's try-a-run card show where it is (NAS-1450).
// Here, not in the lazy dialog, so Home reads it without loading the deploy code. A reload forgets them.

/** One prebuilt agent the guide deployed, by its name. */
export type SeededDeploy =
  | { phase: 'deploying'; title: string; startedAt: number }
  | { phase: 'running' | 'notRunning'; title: string; agentId: string }
  | { phase: 'failed'; title: string; reason: string }

let seededDeploys: ReadonlyMap<string, SeededDeploy> = new Map()
const seededListeners = new Set<() => void>()
function setSeededDeploys(next: ReadonlyMap<string, SeededDeploy>) {
  seededDeploys = next
  for (const fn of seededListeners) fn()
}
export const setSeededDeploy = (name: string, d: SeededDeploy) =>
  setSeededDeploys(new Map(seededDeploys).set(name, d))
export const seededDeployOf = (name: string) => seededDeploys.get(name)
/** Sign-out: the next account never sees this one's deploys. */
export const clearSeededDeploys = () => {
  if (seededDeploys.size) setSeededDeploys(new Map())
}
const subscribeSeeded = (fn: () => void) => {
  seededListeners.add(fn)
  return () => void seededListeners.delete(fn)
}
export const useSeededDeploys = () =>
  useSyncExternalStore(
    subscribeSeeded,
    () => seededDeploys,
    () => seededDeploys,
  )

/** The query a guide with no `starterPack` slot holds instead: never run (`enabled: false`). */
const NO_SLOT_QUERY = {
  queryKey: ['onboarding', 'starter-pack', 'no-slot'],
  queryFn: () => Promise.resolve(null),
}

/** Whether the Starter pack tab is offered: yes without a slot, else the slot's answer, null while it's coming. */
export function useStarterPackOffered(): boolean | null {
  const slot = useSlots().starterPack
  const q = useQuery<unknown>({ ...(slot?.query ?? NO_SLOT_QUERY), enabled: !!slot, retry: false })
  if (!slot) return true
  if (q.isPending) return null
  return slot.offered(q.isError ? undefined : q.data)
}
