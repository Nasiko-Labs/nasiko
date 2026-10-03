import { useNavigate } from '@tanstack/react-router'
import { useCallback } from 'react'
import { z } from 'zod'
import type { FileRoutesByFullPath } from '@edition/routeTree.gen'

/**
 * Search-param building blocks every page schema shares (plan §8 Phase 1). Junk values fall back
 * in the page schemas (`.catch`), never throw, so a stale or hand-edited link still opens.
 * Routes pass the schema itself to `validateSearch`, so typed links take its input type: a key with a
 * fallback value is `.default(v).catch(v)`, since `.catch(v)` alone makes the key required in links.
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

export const isoDate = z.string().refine(isRealDate)
export const text = z.string().trim().min(1).max(200)
// TanStack Router JSON-parses search values, so `?compare=0` arrives as the number 0.
export const flag = z.union([
  z.boolean(),
  z.literal(0).transform(() => false),
  z.literal(1).transform(() => true),
  z.enum(['true', 'false', '1', '0']).transform((v) => v === 'true' || v === '1'),
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
        replace: opts?.replace ?? replaceByDefault,
        resetScroll: false,
      }),
    [navigate, replaceByDefault],
  )
}
