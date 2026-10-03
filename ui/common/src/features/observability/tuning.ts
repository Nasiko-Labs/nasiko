/**
 * Every tuning number for Sessions and Trace, in one place (plan: DX "tuning.ts").
 * Change a value here, not at its call site.
 */

/** Tempo's search limit (`max_duration`, 168 h by default). session/list searches from
 *  start_time to now, so any older start makes every row's trace lookup fail. */
export const TEMPO_MAX_SEARCH_MS = 7 * 86_400_000
/** A 7-day list starts this far inside that limit: the server searches to its own now, which runs ahead of the
 *  page's frozen "now" by up to WINDOW_FREEZE_MS. */
export const TEMPO_SAFETY_MS = 15 * 60_000

/** Live day scan: pages of `SCAN_PAGE_SIZE` from `start_time = day start`, newest first. */
export const SCAN_PAGE_SIZE = 100
export const SCAN_PAGES = 3
/** "Scan 3 more pages" can extend the scan once, to this hard maximum. */
export const SCAN_MAX_PAGES = 6

/** Fleet mode loads this many rows per page ("Load more" fetches the next). */
export const FLEET_PAGE_SIZE = 100
/** A past window pages forward (newest first) at most this far to reach its first row. */
export const FLEET_SEEK_PAGES = 10
/** Fleet mode's "now" stays fixed this long, so Back from a trace reuses the same window and cache. */
export const WINDOW_FREEZE_MS = 10 * 60_000

/** Status checks (TraceDetail per trace) run only for the first rows in sort order. */
export const STATUS_CHECK_ROWS = 25
export const STATUS_CHECK_CONCURRENCY = 4
/** Per session, only the largest traces (by tokens) are checked: sessions can hold dozens. */
export const STATUS_TRACES_PER_SESSION = 3
/** A session that ended this recently may still be receiving spans: don't cache its status. */
export const IN_PROGRESS_MS = 2 * 60_000

/** Retry waste sums at most this many SpanDetail fetches, at most 2 at a time. */
export const RETRY_FETCH_CAP = 10
export const RETRY_FETCH_CONCURRENCY = 2
/** Gaps between retry attempts under this read as "no backoff". */
export const NO_BACKOFF_MS = 100
/** A failed span that ran at least this long reads as a timeout (SpanNode has no status message). */
export const TIMEOUT_MIN_MS = 29_000

/** Waterfall duration labels sit after the bar unless it ends past this % of the axis… */
export const DURATION_LABEL_END_PCT = 82
/** …then before it, unless it also starts before this % (a near-full bar): then inside it. */
export const DURATION_LABEL_START_PCT = 18
/** Span panel Input/Output show this many characters before "Show all". */
export const CONTENT_PREVIEW_CHARS = 600

/** The log drawer keeps (and buffers while paused) at most this many lines. */
export const LOG_LINES_MAX = 500

/** Lanes are hidden below this many loaded sessions ("Not enough sessions to rank"). */
export const LANE_MIN_SESSIONS = 20
/** Per-agent p95 needs this many sessions; below it the fleet p95 is used. */
export const LANE_MIN_PER_AGENT = 5
/**
 * p95 method: nearest-rank (the value at rank ceil(0.95·n) of the sorted sample).
 * "Above p95" is strictly greater, so ties at the threshold stay out of the lane.
 */
export const P95 = 0.95

/** Live mode polls the first page this often, only while Live is on and the tab is visible. */
export const LIVE_POLL_MS = 30_000
export const LIVE_PAGE_SIZE = 25
/** Mock replay reveals one of today's seeded sessions this often. */
export const REPLAY_STEP_MS = 4_000
/** New rows queue behind a pill while the user interacts; they insert after this much idle time. */
export const IDLE_MS = 5_000

/** "Spans still arriving": re-poll TraceDetail this often, at most this many times. */
export const ARRIVAL_POLL_MS = 5_000
export const ARRIVAL_POLLS = 3

/** TokenOps spike clause: the peak day must exceed this multiple of the median day. */
export const SPIKE_FACTOR = 2

/** Below this width shared-layout morphs are off (elements just appear). */
const MORPH_MIN_WIDTH = 640
/** Pre-agreed fallback: set to false to turn every shared-layout (layoutId) morph off. */
const MORPH_ENABLED = true

/** The layoutId to use, or undefined when morphs are off (flag, narrow screens). */
export function morphId(id: string): string | undefined {
  if (!MORPH_ENABLED) return undefined
  if (
    typeof window !== 'undefined' &&
    typeof window.innerWidth === 'number' &&
    window.innerWidth > 0 &&
    window.innerWidth < MORPH_MIN_WIDTH
  )
    return undefined
  return id
}
