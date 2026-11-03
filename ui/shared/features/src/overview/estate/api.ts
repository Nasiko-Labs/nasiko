/**
 * The estate's reads and writes (docs/requirements/overview.md OV-1..OV-3). A bare 404 means the server has no such
 * route: the estate read resolves to null (the page builds its smaller map, `fallback.ts`), problem writes are left
 * out (no Assign, Snooze or Mark handled), and pins live in this browser instead.
 */
import { useMutation, useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query'
import { useCallback, useSyncExternalStore } from 'react'
import { apiData, apiFetch } from '@nasiko/data-access/client'
import { isEndpointAbsent } from '@nasiko/data-access/detect'
import { DRAFT_PREFIX } from '@nasiko/app-core/draftKeys'
import type { Range } from '../search'
import {
  estateSchema,
  pinsSchema,
  problemResponseSchema,
  type Estate,
  type Problem,
  type ProblemPatch,
} from './types'

export const estateKeys = {
  all: ['overview', 'estate'] as const,
  estate: (range: Range) => ['overview', 'estate', range] as const,
  pins: ['overview', 'pins'] as const,
}

async function optional<T>(load: () => Promise<T>): Promise<T | null> {
  try {
    return await load()
  } catch (err) {
    if (isEndpointAbsent(err)) return null
    throw err
  }
}

/** OV-1. `data === null`: the server has no estate route. */
export function useEstate(range: Range, enabled = true) {
  return useQuery({
    queryKey: estateKeys.estate(range),
    queryFn: ({ signal }) =>
      optional(() =>
        apiData<Estate>(`/api/overview/estate?range=${range}`, { signal, schema: estateSchema }),
      ),
    enabled,
    // The map is live: problems and requests move while the page is open.
    refetchInterval: 60_000,
    refetchIntervalInBackground: false,
    placeholderData: (prev) => prev,
  })
}

/** Writes a problem into every cached estate (each range holds the same problems). */
function patchCached(qc: QueryClient, id: string, change: (p: Problem) => Problem) {
  qc.setQueriesData<Estate | null>({ queryKey: estateKeys.all }, (e) =>
    e && 'problems' in e
      ? { ...e, problems: e.problems.map((p) => (p.id === id ? change(p) : p)) }
      : e,
  )
}

/** Applies OV-2's body to a problem the way the server will, so the map changes before it answers. */
export function applyPatch(p: Problem, patch: ProblemPatch, now: number): Problem {
  return {
    ...p,
    ...('owner_id' in patch ? { owner_id: patch.owner_id ?? null } : {}),
    ...('snoozed_until' in patch ? { snoozed_until: patch.snoozed_until ?? null } : {}),
    ...('handled' in patch
      ? { handled_at: patch.handled ? new Date(now).toISOString() : null }
      : {}),
  }
}

/** OV-2, optimistic: the change shows at once and rolls back if the server refuses it. */
export function useProblemAction() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, patch }: { id: string; patch: ProblemPatch }) =>
      apiFetch(`/api/overview/problems/${encodeURIComponent(id)}`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(patch),
        schema: problemResponseSchema,
      }).then((r) => r.data),
    onMutate: async ({ id, patch }) => {
      await qc.cancelQueries({ queryKey: estateKeys.all })
      const before = qc.getQueriesData<Estate | null>({ queryKey: estateKeys.all })
      patchCached(qc, id, (p) => applyPatch(p, patch, Date.now()))
      return { before }
    },
    onError: (_e, _v, ctx) => ctx?.before.forEach(([key, data]) => qc.setQueryData(key, data)),
    onSuccess: (p) => patchCached(qc, p.id, () => p),
  })
}

// Pins without OV-3: this browser's, per user, under the drafts prefix so sign-out clears them.
const localKey = (user: string) => `${DRAFT_PREFIX}overview-pins:${user}`
const listeners = new Set<() => void>()
function readLocal(user: string): string[] | null {
  try {
    const raw = globalThis.localStorage?.getItem(localKey(user))
    const v: unknown = raw ? JSON.parse(raw) : null
    return Array.isArray(v) && v.every((x) => typeof x === 'string') ? v : null
  } catch {
    return null
  }
}
/** The last value read per user, so `useSyncExternalStore` sees a stable snapshot. */
const snapshots = new Map<string, { raw: string | null; value: string[] | null }>()
function localSnapshot(user: string): string[] | null {
  let raw: string | null = null
  try {
    raw = globalThis.localStorage?.getItem(localKey(user)) ?? null
  } catch {
    raw = null
  }
  const hit = snapshots.get(user)
  if (hit && hit.raw === raw) return hit.value
  const value = readLocal(user)
  snapshots.set(user, { raw, value })
  return value
}
function writeLocal(user: string, pins: string[]) {
  try {
    globalThis.localStorage?.setItem(localKey(user), JSON.stringify(pins))
  } catch {
    // Blocked storage: the pins last for this page only.
  }
  listeners.forEach((l) => l())
}
const subscribe = (l: () => void) => {
  listeners.add(l)
  return () => void listeners.delete(l)
}

export interface Pins {
  /** The pinned card ids in order; null until the viewer has pinned or unpinned anything (defaults apply). */
  pins: string[] | null
  /** OV-3 is on this server: pins follow the viewer to other browsers. */
  synced: boolean
  isPending: boolean
  set(next: string[]): void
}

/** OV-3 when the server has it, else this browser's storage. */
export function usePins(user: string): Pins {
  const qc = useQueryClient()
  const q = useQuery({
    queryKey: estateKeys.pins,
    queryFn: ({ signal }) =>
      optional(() => apiFetch('/api/me/overview-pins', { signal, schema: pinsSchema })),
    staleTime: 5 * 60_000,
  })
  const local = useSyncExternalStore(
    subscribe,
    () => localSnapshot(user),
    () => null,
  )
  const save = useMutation({
    mutationFn: (pins: string[]) =>
      apiFetch('/api/me/overview-pins', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ pins }),
        schema: pinsSchema,
      }),
    onMutate: async (pins) => {
      await qc.cancelQueries({ queryKey: estateKeys.pins })
      const before = qc.getQueryData(estateKeys.pins)
      qc.setQueryData(estateKeys.pins, { pins })
      return { before }
    },
    onError: (_e, _v, ctx) => qc.setQueryData(estateKeys.pins, ctx?.before),
  })
  const absent = q.data === null || (q.isError && isEndpointAbsent(q.error))
  const { mutate } = save
  const set = useCallback(
    (next: string[]) => (absent ? writeLocal(user, next) : mutate(next)),
    [absent, mutate, user],
  )
  return {
    pins: absent ? local : (q.data?.pins ?? null),
    synced: !absent,
    isPending: q.isPending,
    set,
  }
}
