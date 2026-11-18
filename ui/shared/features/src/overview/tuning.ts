/** Every Overview tuning number (plans/feat-overview.md §5.2): thresholds are relative to each agent's own history. */

const HOUR = 3_600_000
export const DAY = 24 * HOUR

/** Cost: 7-day spend up more than this vs the previous 7 days is Watch. */
export const COST_RISE_PCT = 50
/** Cost: cost per operation above this multiple of its 30-day average is Watch. */
export const COST_PER_OP_FACTOR = 2
/** Cost: the driver of a day at or above this multiple of a typical day, within the last week, needs action. */
export const SPIKE_ACTION_FACTOR = 3
/** Spike days are looked for in the last this-many days of the 30-day series. */
export const SPIKE_LOOKBACK_DAYS = 7

/** Activity: operations down more than this vs the previous 7 days is Watch. */
export const ACTIVITY_DROP_PCT = 70

/** Latency: p95 up more than this vs the previous 7 days is Watch. */
export const P95_RISE_PCT = 50

/** Comparisons need this many operations in the previous window, so "▲ 400%" on noise never rates an agent. */
export const MIN_BASE_OPS = 20

/** Activity: an agent younger than this isn't called idle. */
export const IDLE_MIN_AGE_DAYS = 7
/** Spike detection needs at least this many days to have a typical day. */
export const MIN_SPIKE_DAYS = 3

/** Top cost drivers in the Spend card: one per chart colour, the rest fold into Other. */
export const TOP_DRIVERS = 5

/**
 * Failed sessions fetched for Needs you. The count it reports is the server's
 * `matching_count` — every match in the window — so this only bounds how many
 * agent names the row can name, never the number it states.
 */
export const FAILED_SESSION_ROWS = 25

/** Needs you shows this many rows before its list scrolls (design review 2A). */
export const NEEDS_VISIBLE_ROWS = 5
/** Fleet health lists at most this many rated agents, Watch first. */
export const HEALTH_LIST_ROWS = 5
/** Fleet health draws a square per agent up to this many agents (4 rows in the card); past it, one bar sized by the
 * rating counts, so the card stays the same height at any fleet size (NAS-1519). */
export const HEALTH_SQUARES_MAX = 100
/** Recent sessions shows this many rows — and asks the server for exactly that. */
export const RECENT_SESSION_ROWS = 5
/** Saved tokens by agent: named bars before Other; 7 fill the card beside 5 Recent sessions rows. */
export const SAVED_BARS = 7
/** Savings rows asked for: the server's maximum (its default of 25 would leave most agents out of the ranking). */
export const SAVINGS_ROWS = 200
/** How often Needs you's "Checked …" label re-ages on an open tab. */
export const CHECKED_TICK_MS = 30_000
/**
 * How often an open map refetches the estate. Each read is the platform's heaviest query set, and every open Home tab
 * polls, so a minute put the Free plane's database at its limit near 11K open tabs; five minutes leaves room for 50K.
 */
export const ESTATE_REFRESH_MS = 5 * 60_000
/** Returning to the page after this long refetches the inbox (design review 8A, the TokenOps rule). */
export const RETURN_REFRESH_MS = 60_000
/** Links to Sessions open on the Overview's own 7-day window: a 30-day window runs past Tempo's 7-day search limit (QA ISSUE-003). */
export const LAST_WEEK = { preset: '7d' } as const
/** "Did you know?" turns to the next tip after this long (paused on hover or focus, never under reduced motion). */
export const TIP_ROTATE_MS = 6_000
/** The coding harness preview's steps (Today, Day 1, Week 1, Month 1) each play this long: 12 s in all (canvas D). */
export const PREVIEW_STEP_MS = 3_000
/** Once the preview ends, Home comes back after this long unless the viewer stays (user 2026-11-06: 5 s). */
export const PREVIEW_HOLD_MS = 5_000
/** The preview's clock moves on in steps this long (20 frames a second). */
export const PREVIEW_TICK_MS = 50
/** The try-a-run card (components/TryAgentRun.tsx): "Agent completed" stays on the button this long before it resets. */
export const TRY_RUN_COMPLETED_MS = 2_000
