import { z } from 'zod/mini'
import { sharedSearchSchema } from '@nasiko/app-core/shell/context'
import { fallback, flag, isoDate, opt, text } from '@nasiko/ui/lib/search'

/**
 * Everything the TokenOps page shows is driven by these URL search params
 * (plan: "Filters live in the URL"). Junk values fall back to defaults instead of
 * throwing, so a hand-edited or stale link still opens the page.
 */
const SORTS = ['cost', 'tokens', 'operations', 'latency', 'hours', 'name'] as const
export type SortKey = (typeof SORTS)[number]

/**
 * Section ids the `open` param names (links from other pages: the page scrolls to that section on arrival). The page
 * has no expanders any more; `perf`, `month` and `metrics` are kept so older links still parse.
 */
export const DISCLOSURES = ['spend', 'optimize', 'drivers', 'perf', 'month', 'metrics'] as const
export type Disclosure = (typeof DISCLOSURES)[number]

/** Normalise `open` to a canonical CSV (kept a string so the URL stays `open=spend,drivers`). */
function parseOpen(raw: string): string | undefined {
  if (raw.trim() === 'all') return DISCLOSURES.join(',')
  const ids = raw.split(',').map((s) => s.trim())
  const kept = DISCLOSURES.filter((d) => ids.includes(d))
  return kept.length ? kept.join(',') : undefined
}

/** The open disclosures as a set. */
export function openSet(open: string | undefined): Set<Disclosure> {
  return new Set(
    (open ?? '')
      .split(',')
      .filter((s): s is Disclosure => (DISCLOSURES as readonly string[]).includes(s)),
  )
}

/** The shared context (app/shell/context.ts) plus TokenOps' own keys. */
export const tokenopsSearchSchema = z.extend(sharedSearchSchema, {
  view: fallback(z.enum(['agent', 'workflow']), 'agent'),
  sort: fallback(z.enum(SORTS), 'cost'),
  /** EE `tokenopsOrgUnitFilter` slot: a unit lead or admin's chosen unit, "My usage" when absent.
   * Ignored by OSS and by anyone the unit doesn't belong to — the server is the real gate. */
  org_unit: opt(z.string()),
  /** Truncated, never discarded: a long query must not clear the box while typing. */
  q: opt(
    z.pipe(
      z.string(),
      z.transform((s) => s.slice(0, 200)),
    ),
  ),
  /** Selected day for the day panel (UTC date). */
  day: opt(isoDate),
  traces: opt(flag),
  /** Sections to land on: CSV of DISCLOSURES, or `all`. Unknown ids are dropped. */
  open: opt(z.pipe(z.string(), z.transform(parseOpen))),
  // Mock-only (read by the mock bootstrap from the URL): kept as written, as on /optimization, so a scenario such as
  // `?mock=desktop-running` survives the router's search validation instead of being dropped.
  mock: opt(text),
})

export type TokenopsSearch = z.infer<typeof tokenopsSearchSchema>
