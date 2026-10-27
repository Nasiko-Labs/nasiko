/**
 * Agent detail URL state (plans/feat-agent-centric.md, DX URL keys). The detail's `tab` plus the hub's keys:
 * TokenOps' window (`preset`, `from`, `to`), `compare` (off unless `true`, replaced), `slice` (a bucket's
 * `<ISO start>/<ISO end>`, pushed, as Optimization's) and `marker` (`<kind>:<record id>`, replaced when dropped).
 *
 * The route's schema loads with the app shell, so this module imports only zod and `lib/search` (the
 * `shell-import-chain` learning); the hub's code stays in its lazy chunk.
 */
import { z } from 'zod/mini'
import { sharedSearchSchema } from '@nasiko/app-core/shell/context'
import { opt, PRESETS, text, type Preset } from '@nasiko/ui/lib/search'

export const hubSearchSchema = z.extend(sharedSearchSchema, {
  /** Optional here (TokenOps defaults it in the schema): an agent link stays `/agents/<id>` until the window changes. */
  preset: opt(z.enum(PRESETS)),
  /** Unknown tab values are kept as strings so the page can fall back to Overview with replace-history. */
  tab: opt(z.string().check(z.trim(), z.maxLength(40))),
  slice: opt(text),
  marker: opt(text),
  // Mock-only (read by the mock bootstrap from the URL): kept as written, so `?mock=hub-not-visible` survives the
  // router's search validation (the `route-search-drops-mock` learning).
  mock: opt(text),
})

export type HubSearch = z.infer<typeof hubSearchSchema>

/** The hub's window when the URL names none: TokenOps' default (DX "URL keys"). */
export const HUB_DEFAULT_PRESET: Preset = '30d'

/** The hub's compare is off by default (X2): only an explicit `compare=1` turns it on, unlike TokenOps'. */
export const hubCompareOn = (s: { compare?: boolean }) => s.compare === true
