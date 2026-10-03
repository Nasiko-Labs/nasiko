/**
 * Shared page context carried across TokenOps, Sessions and Session trace in the URL.
 *
 * URL contract (plan: DX "URL contract"). Every param is optional; junk falls back.
 *
 * | Param     | Type                              | Default | Read by                  | Carried across routes |
 * |-----------|-----------------------------------|---------|--------------------------|-----------------------|
 * | preset    | 24h·7d·30d·mtd·last-month·custom   | 30d (Sessions: 7d) | TokenOps, Sessions       | yes, except into Sessions and TokenOps (`withoutWindow`) |
 * | from, to  | YYYY-MM-DD (custom window)        | —       | TokenOps, Sessions       | as preset |
 * | agent     | TokenOps: UUID or raw name; Sessions: raw name | — | all             | yes |
 * | provider  | string                            | —       | TokenOps (Sessions: shown as not applied) | yes |
 * | model     | string                            | —       | TokenOps (Sessions: shown as not applied) | yes |
 * | compare   | 0 · 1                             | 1       | TokenOps Δs + narrative  | yes |
 * | anchor    | YYYY-MM-DD (mock mode only)       | —       | the mock seed's "now"    | yes |
 * | mock      | tempo-down·empty·trace-503·trace-500·scan-fail·server-down·logout-unavailable (mock only) | — | MSW | yes |
 * | demo      | 1 (presenter card)                | —       | all                      | yes |
 * | open      | CSV of spend,drivers,perf,month,metrics, or all | — | TokenOps disclosures | no |
 * | day       | YYYY-MM-DD (UTC)                  | —       | TokenOps day panel, Sessions day mode | no (explicit links) |
 * | status    | failed · ok                       | —       | Sessions                 | no |
 * | lane      | failing · slow · costly           | —       | Sessions                 | no |
 * | sort      | TokenOps: cost…; Sessions: cost·time | per page | both                | no |
 * | live      | paused (mock/live Sessions)       | —       | Sessions                 | no |
 * | trace     | trace id                          | failing, else most tokens | Session trace | no |
 * | span      | HEX span id (never the base64 id) | failing, else most tokens, else slowest | Session trace | no |
 *
 * Every id that goes into a path or query is `encodeURIComponent`-ed by the router.
 */
import { z } from 'zod'
import { flag, isoDate, PRESETS, text } from '@/lib/search'

const SHARED_KEYS = [
  'preset',
  'from',
  'to',
  'agent',
  'provider',
  'model',
  'compare',
  'anchor',
  'mock',
  'demo',
] as const

/** The shared keys' schema, merged into each page's own search schema. */
export const sharedSearchSchema = z.object({
  preset: z.enum(PRESETS).default('30d').catch('30d'),
  from: isoDate.optional().catch(undefined),
  to: isoDate.optional().catch(undefined),
  agent: text.optional().catch(undefined),
  provider: text.optional().catch(undefined),
  model: text.optional().catch(undefined),
  compare: flag.optional().catch(undefined),
  anchor: isoDate.optional().catch(undefined),
  mock: z
    .enum([
      'tempo-down',
      'empty',
      'trace-503',
      'trace-500',
      'scan-fail',
      'server-down',
      'logout-unavailable',
    ])
    .optional()
    .catch(undefined),
  demo: flag.optional().catch(undefined),
})

export type SharedSearch = z.infer<typeof sharedSearchSchema>

/** Keep only the shared keys (for nav links and cross-page jumps). */
export function pickShared(search: Record<string, unknown>): Partial<SharedSearch> {
  const out: Record<string, unknown> = {}
  for (const k of SHARED_KEYS) if (search[k] !== undefined) out[k] = search[k]
  return out as Partial<SharedSearch>
}

/** Links into Sessions and TokenOps: the shared keys minus the window, so each starts on its own (7d, 30d). */
export function withoutWindow(search: Record<string, unknown>): Partial<SharedSearch> {
  const { preset, from, to, ...rest } = pickShared(search)
  return rest
}

/** Compare period defaults to on; only an explicit `compare=0` turns it off. */
export const compareOn = (s: { compare?: boolean }) => s.compare !== false

/** Presenter mode: which of the three demo steps the current URL is on (null = none). */
export function presenterStep(pathname: string, search: Record<string, unknown>): number | null {
  if (pathname === '/tokenops') return 0
  if (pathname === '/sessions' || pathname === '/sessions/') return search.day ? 1 : null
  if (pathname.startsWith('/sessions/')) return 2
  return null
}
