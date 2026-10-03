import { z } from 'zod'
import { CHAT_PAGE_VARIANT_KEYS, CHAT_SCENARIO_KEYS } from './scenarioKeys'

/**
 * URL state for Chat (plan §5; v1b §5.1). The rail filter is local state, not a param.
 * - `agent` keeps its presence whatever its value: the router's JSON search parser turns
 *   `?agent=123` into a number and `?agent=true` into a boolean, and a blank, oversized or
 *   non-string value must show the banner, never fall back to another kind of chat (DX-1, G-16).
 * - `auto` arrives as the number 1 through that parser (`main.tsx`), or as '1'.
 * `mock` and `debug` are dev/mock aids; they survive the create and opt-in navigations (DX-A2).
 */
export const chatSearchSchema = z.object({
  agent: z.unknown().optional(),
  auto: z
    .union([z.literal(1), z.literal('1')])
    .optional()
    .catch(undefined),
  // Kept raw so an unknown value can be named in the dev banner (DX-7); isChatScenario checks it.
  mock: z.string().max(200).optional().catch(undefined),
  debug: z.literal('turn').optional().catch(undefined),
})
export type ChatSearch = z.infer<typeof chatSearchSchema>
/** The dev aids every chat link carries (DX-A2). */
export type ChatCarry = Pick<ChatSearch, 'mock' | 'debug'>

declare module '@tanstack/react-router' {
  interface HistoryState {
    /** The Waiting row's request, carried in history state so the opened chat focuses its card (v1c M4). */
    waitingRequest?: string
  }
}

/** `/chat?auto=1`: the routed empty state (UC1). */
export const isAuto = (s: Pick<ChatSearch, 'auto'>) => s.auto === 1 || s.auto === '1'

/** The `agent` value as text for keys and the banner (a number or array shows as typed). */
export function agentParamText(v: unknown): string {
  if (v === undefined) return ''
  if (typeof v === 'string') return v
  // From the router's JSON search parser: plain values, so stringify can't throw.
  return typeof v === 'object' ? JSON.stringify(v) : String(v)
}

/** A `?mock=` value Chat knows (the mock handlers read the same list). */
const isChatScenario = (v: string | undefined) =>
  !!v && (CHAT_SCENARIO_KEYS as readonly string[]).includes(v)

/** `?mock=` is a comma list (v1c DX1): at most one stream scenario plus any page variants. */
export const mockEntries = (v: string | undefined): string[] =>
  (v ?? '')
    .split(',')
    .map((e) => e.trim())
    .filter(Boolean)

/** The entries Chat doesn't know, for the dev banner. */
export const unknownMockEntries = (v: string | undefined): string[] =>
  mockEntries(v).filter(
    (e) => !isChatScenario(e) && !(CHAT_PAGE_VARIANT_KEYS as readonly string[]).includes(e),
  )
