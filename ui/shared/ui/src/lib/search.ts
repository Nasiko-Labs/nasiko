import { useNavigate } from '@tanstack/react-router'
import { useCallback } from 'react'
import { z } from 'zod/mini'
import type { FileRoutesByFullPath } from '@edition/routeTree.gen'

/**
 * Search-param building blocks every page schema shares (plan §8 Phase 1). Junk values fall back
 * in the page schemas (`.catch`), never throw, so a stale or hand-edited link still opens.
 * Routes pass the schema itself to `validateSearch`, so typed links take its input type: a key with a
 * fallback value is `fallback(s, v)` (`.default(v).catch(v)`), since a catch alone makes the key required in links.
 * zod/mini, not zod: these schemas load with the shell, and full zod's JSON-schema code costs it ~4 KB gz.
 */

/** Time-window presets (TokenOps, Sessions, Harnesses). */
export const PRESETS = ['24h', '7d', '30d', 'mtd', 'last-month', 'custom'] as const
export type Preset = (typeof PRESETS)[number]

/** A real calendar date: `2026-13-01` and `2026-02-30` are rejected, not coerced. */
export function isRealDate(s: string): boolean {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(s)) return false
  const d = new Date(`${s}T00:00:00Z`)
  return !Number.isNaN(d.getTime()) && d.toISOString().slice(0, 10) === s
}

/** `.optional().catch(undefined)`: a junk value drops the key. */
export const opt = <T extends z.ZodMiniType>(s: T) => z.catch(z.optional(s), undefined)
/** `.default(v).catch(v)`: links may omit the key, and junk falls back to `v`. */
export const fallback = <T extends z.ZodMiniType>(s: T, v: z.util.NoUndefined<z.output<T>>) =>
  z.catch(z._default(s, v), v)

export const isoDate = z.string().check(z.refine(isRealDate))
export const text = z.string().check(z.trim(), z.minLength(1), z.maxLength(200))
// TanStack Router JSON-parses search values, so `?compare=0` arrives as the number 0.
export const flag = z.union([
  z.boolean(),
  z.pipe(
    z.literal(0),
    z.transform(() => false),
  ),
  z.pipe(
    z.literal(1),
    z.transform(() => true),
  ),
  z.pipe(
    z.enum(['true', 'false', '1', '0']),
    z.transform((v) => v === 'true' || v === '1'),
  ),
])

/**
 * A page's `setSearch`: merges a patch into route `from`'s search (pass `Route.fullPath`), without
 * scrolling. Pushes history unless `replaceByDefault` (or the call's `replace`) says to replace.
 */
export function useSetSearch<S extends object>(
  from: keyof FileRoutesByFullPath,
  replaceByDefault = false,
) {
  const navigate = useNavigate({ from })
  return useCallback(
    (patch: Partial<S>, opts?: { replace?: boolean }) =>
      void navigate({
        search: (prev) => ({ ...prev, ...patch }),
        // A search change on a detail page keeps the list it was opened from (lib/siblings.ts), and nothing else
        // (an unchanged search with the same list writes no history entry).
        state: (prev) => (prev.siblings ? { siblings: prev.siblings } : {}),
        replace: opts?.replace ?? replaceByDefault,
        resetScroll: false,
      }),
    [navigate, replaceByDefault],
  )
}
