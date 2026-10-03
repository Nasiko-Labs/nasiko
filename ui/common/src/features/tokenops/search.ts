import { z } from 'zod'
import { sharedSearchSchema } from '@/app/shell/context'
import { flag, isoDate } from '@/lib/search'

/**
 * Everything the TokenOps page shows is driven by these URL search params
 * (plan: "Filters live in the URL"). Junk values fall back to defaults instead of
 * throwing, so a hand-edited or stale link still opens the page.
 */
const SORTS = ['cost', 'tokens', 'operations', 'latency', 'hours', 'name'] as const
export type SortKey = (typeof SORTS)[number]

/** TokenOps disclosure ids (the `open` param). */
export const DISCLOSURES = ['spend', 'optimise', 'drivers', 'perf', 'month', 'metrics'] as const
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

/** `open` with one disclosure toggled (undefined when none are left). */
export function toggleOpen(open: string | undefined, id: Disclosure): string | undefined {
  const set = openSet(open)
  if (set.has(id)) set.delete(id)
  else set.add(id)
  return set.size ? DISCLOSURES.filter((d) => set.has(d)).join(',') : undefined
}

/** The shared context (app/shell/context.ts) plus TokenOps' own keys. */
export const tokenopsSearchSchema = sharedSearchSchema.extend({
  view: z.enum(['agent', 'workflow']).default('agent').catch('agent'),
  sort: z.enum(SORTS).default('cost').catch('cost'),
  /** Truncated, never discarded: a long query must not clear the box while typing. */
  q: z
    .string()
    .transform((s) => s.slice(0, 200))
    .optional()
    .catch(undefined),
  /** Selected day for the day panel (UTC date). */
  day: isoDate.optional().catch(undefined),
  traces: flag.optional().catch(undefined),
  more: flag.optional().catch(undefined),
  /** Open disclosures: CSV of DISCLOSURES, or `all`. Unknown ids are dropped. */
  open: z.string().transform(parseOpen).optional().catch(undefined),
})

export type TokenopsSearch = z.infer<typeof tokenopsSearchSchema>
