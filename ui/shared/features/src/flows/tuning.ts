/** Every flows tuning number (plans/feat-flows.md). */

/** The list reads back to the window start in pages of this size (A2). */
export const LIST_PAGE = 100
/** …and stops at this many flows (F16, A2). */
export const LIST_CAP = 500

/** The page clock while calls run (elapsed seconds, the running edge)… */
export const TICK_MS = 1_000
/** …and while the flow waits on a human (waits read in whole minutes, `fmtWait`). */
export const PAUSED_TICK_MS = 15_000
/** The list's search commits after this pause in typing. */
export const SEARCH_DEBOUNCE_MS = 300
/** `resolveWindow` floors "now" to the minute: a window ending within this of now is still open. */
export const LIVE_EDGE_MS = 60_000

/** A running flow is re-read this often while the tab is visible (F13). */
export const RUNNING_POLL_MS = 3_000
/** A paused flow (waiting on a human) is re-read this often (F13). */
export const PAUSED_POLL_MS = 15_000
/** A running or paused flow with nothing new for this long (a stuck row, a long wait) is re-read… */
export const STUCK_AFTER_MS = 30 * 60_000
/** …only this often. */
export const STUCK_POLL_MS = 30_000

/** Agent-to-agent calls read from the trace: span details at most this many at a time (A1)… */
export const SPAN_DETAIL_CONCURRENCY = 4
/** …and at most this many per flow, counted across every re-read (A1, O4). */
export const TRACE_CALL_CAP = 30
/** While the flow runs, the trace is re-read on every 4th flow poll (O4). */
export const TRACE_REREAD_MS = RUNNING_POLL_MS * 4
/** One more trace read this long after the flow ends: spans land in Tempo a few seconds late (O4). */
export const TRACE_FINAL_READ_MS = 10_000

/** An overdue final trace read runs this soon (TanStack Query reads 0 as "no interval"). */
export const TRACE_OVERDUE_MS = 1_000

/** A recorded step and a trace call for the same agent pair when their starts are this close (A5). */
export const MATCH_WINDOW_MS = 1_000

/** An agent call ending this soon after `completed_at` is the normal write order, not an early completion (O5). */
export const FINISH_TOLERANCE_MS = 1_000
/** "Finishing" (O5) lasts at most this long after the server's `completed_at`. */
export const FINISHING_MAX_MS = 120_000

/** Lanes shown before "Show N more agents" (F27). */
export const LANE_CAP = 12
/** Concurrent sub-rows in one lane before "+N" (F27). */
export const SUBROW_CAP = 3
/** Past this many calls the swimlane panel opens on its Table view (F27). */
export const TABLE_FIRST_CALLS = 200
/** Per-agent colours (F21): DESIGN.md's five Two-tone series; the sixth agent on takes Other. */
export const AGENT_COLOURS = 5
