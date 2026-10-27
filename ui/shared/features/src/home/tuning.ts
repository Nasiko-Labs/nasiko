/** The home's numbers (plans/feat-agent-centric.md DX: caps and sizes live in the feature's `tuning.ts`). */
export { RETURN_REFRESH_MS } from '@nasiko/features/overview/tuning'
/** Skeleton rows while the persona or the directory is pending (R2-12: neutral, never an empty state). */
export const SKELETON_ROWS = 6
/** Height of one skeleton row while the table loads. */
export const SKELETON_ROW_PX = 40
/** Savings rows asked for (the server's maximum; its default of 25 would leave most agents "not reported"). */
export const SAVINGS_ROWS = 200
/** A touch-sized target on coarse pointers (DESIGN.md: 44 px). */
export const TOUCH = 'pointer-coarse:min-h-11'
/** Budget use is re-read this often while the tab is visible (PR2 "Live budgets"; TanStack pauses it when hidden). */
export const BUDGET_POLL_MS = 60_000
/** Per-row reads (sparkline series, build records) in flight at once (eng "Home execution table"). */
export const ROW_READS = 4
/** A row's series and builds stay fresh this long, so scrolling back doesn't read them again. */
export const ROW_READ_STALE_MS = 5 * 60_000
/** The budget column's "updated …" stamp moves this often. */
export const STAMP_TICK_MS = 30_000
/** A row must stay on screen this long before its reads start, so rows scrolled straight past cost nothing. */
export const ROW_DWELL_MS = 250
/** Rows this close to the viewport count as on screen, so their cells are filled by the time they scroll in. */
export const ROW_MARGIN = '200px 0px'
/** Below this width the rows are two lines each (design "Responsive": < 640 px). */
export const PHONE_QUERY = '(max-width: 639px)'
/** A row's sparkline box (px). */
export const SPARK_W = 64
export const SPARK_H = 20
/** Rows a phone list draws before "Show N more" (decision #50). */
export const PHONE_ROWS = 25
/** The chart band's By agent bars before Other (PR2d). */
export const BAND_BARS = 10
/** Saved tokens by agent's bars before Other: short, so it sits level with the health panel beside it. */
export const SAVED_BARS = 5
/** Agents stacked in Over time: the chart kit's five series colours (`seriesAt`). */
export const BAND_STACK = 5
/** The chart band's placeholder while its chunk loads. */
export const BAND_SKELETON_PX = 420
/** The summary bento's placeholder until the rows answer (two tile rows), so Needs you below it doesn't jump. */
export const SUMMARY_SKELETON_PX = 216
