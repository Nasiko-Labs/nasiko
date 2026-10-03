/**
 * Every tunable number on the Harnesses page, in one place (plan §9). Adoption
 * thresholds are a deferred policy (TODOS.md); changing one should touch only this file.
 */

/** A unit needs this many in-scope developers before it can be the "lowest adoption" callout. */
export const CALLOUT_MIN_SCOPE_DEVS = 3
/** Above this share of unpriced turns, a harness's cost shows "unpriced" instead of a number. */
export const MOSTLY_UNPRICED_RATIO = 0.5
/** Below this width the breakdown renders one card per row and the breadcrumb collapses. */
export const NARROW_BREAKPOINT_PX = 640
/** Individual view: recent sessions shown before "Show all". */
export const RECENT_SESSIONS_SHOWN = 8
/** Live fallback: sessions requested from the own-only session list. */
export const LIVE_SESSION_LIMIT = 20
/** A developer is active with at least this many turns in the window. */
export const ACTIVE_MIN_TURNS = 1
/** Longest custom window, in days: a longer `from` is clamped (charts render one point per day). */
export const MAX_WINDOW_DAYS = 366
/** A growth callout needs at least this turns change, so it never reads "up 0%". */
export const CALLOUT_MIN_GROWTH_PCT = 1
/** Rows requested per page for developer lists (group_by=user). */
export const USER_PAGE_LIMIT = 500
/** Live fallback: /api/agents page size (the server clamps limit to 100) and the paging cap. */
export const AGENT_PAGE_SIZE = 100
export const AGENT_PAGE_CAP = 2000

/** Known harnesses, in display order, with their chart fill and edge tokens (plan §6, G7; DESIGN.md "Charts"). */
export const HARNESSES = [
  { id: 'claude', name: 'Claude Code', color: 'var(--chart-1)', edge: 'var(--chart-1-edge)' },
  { id: 'codex', name: 'Codex', color: 'var(--chart-2)', edge: 'var(--chart-2-edge)' },
  { id: 'opencode', name: 'OpenCode', color: 'var(--chart-3)', edge: 'var(--chart-3-edge)' },
  { id: 'cursor', name: 'Cursor', color: 'var(--chart-4)', edge: 'var(--chart-4-edge)' },
] as const
