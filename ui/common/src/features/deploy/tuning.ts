/** Every number the deploy and build pages use (plans/feat-deploy.md). */

/** Builds list page size; the server's default is 20 and its `total` is the page length (D-6). */
export const BUILDS_PAGE_SIZE = 20
/** In-progress rows and pinned builds refresh at this pace while any build runs and the tab is visible (§6). */
export const BUILDS_REFRESH_MS = 5_000
/** A dropped build stream falls back to polling the upload status at the server's own pace (3 s DB poll). */
export const STREAM_FALLBACK_POLL_MS = 3_000
/** After the build succeeds, the agent's status is checked until it's running or failed (§5 outcomes). */
export const AGENT_SETTLE_POLL_MS = 3_000
/** Failure reasons load per failed row on the visible page, this many at a time (design review 2). */
export const REASON_READS = 4
/** A build past this reads "Taking longer than usual" (design review 9). */
export const SLOW_BUILD_MS = 5 * 60_000
/** The worker's runtime build timeout (`build_worker.rs`). */
export const BUILD_TIMEOUT_MIN = 30
/** The elapsed clock on the current step ticks once a second (design review 13: no animated digits). */
export const ELAPSED_TICK_MS = 1_000
/** The Connect GitHub popup: poll the connection every 2 s, at most 90 times (3 min), stop when the popup closes (§4.2). */
export const GITHUB_POLL_MS = 2_000
export const GITHUB_POLL_TRIES = 90
/** The background follower (eng review R8): one uploads read covers every followed build, at this pace. */
export const FOLLOW_POLL_MS = 5_000
export const FOLLOW_UPLOADS_LIMIT = 20
/** A followed build not seen finishing by then is dropped (the worker's build timeout is 30 min). */
export const FOLLOW_MAX_MS = 45 * 60_000
/** Failed reads double the follower's pace up to this (a stopped server or a missing route). */
export const FOLLOW_BACKOFF_MAX_MS = 60_000
/** The Build page re-reads the agent after the image is built for at most this long, then stops (it may never settle). */
export const AGENT_SETTLE_MAX_MS = 5 * 60_000
/** Failure reasons stay cached this long after their rows leave the page. */
export const REASON_GC_MS = 30 * 60_000
/** Whether the server has a GitHub OAuth app rarely changes; the repository list is re-read after a minute. */
export const GITHUB_CONFIGURED_STALE_MS = 5 * 60_000
export const GITHUB_REPOS_STALE_MS = 60_000
/** The Connect GitHub popup window. */
export const GITHUB_POPUP = { name: 'openruntime-github', width: 720, height: 760 } as const
