/** Timing knobs for the agent pages (plan §6.2), in one place. */

/** Poll interval while something is deploying, restarting or rolling back. */
export const POLL_MS = 5_000
/** A restart or roll back stops being watched after this long. */
export const WATCH_CAP_MS = 3 * 60_000
/**
 * Restart writes `running` before it returns (admin/routes.rs:568-575), and the roll back task
 * writes `deploying` asynchronously; a result earlier than this after the action isn't trusted.
 */
export const GRACE_MS = 10_000
/** Consecutive Running polls needed before a restart counts as done. */
export const STABLE_POLLS = 2

export const DIRECTORY_STALE_MS = 5 * 60_000
export const DETAIL_STALE_MS = 30_000
export const DEPLOYMENT_STALE_MS = 60_000
export const LIVE_CARD_STALE_MS = 5 * 60_000
export const ERROR_LOGS_STALE_MS = 60_000

/** `GET /api/agents` clamps `limit` to 100 (catalog/routes.rs). */
export const AGENTS_PAGE = 100
/** Safety stop for the paging loop (plan: up to 300 agents). */
export const AGENTS_MAX_PAGES = 20
/** Recent sessions are filtered from the first page of session/list (limit ≤ 100). */
export const SESSIONS_SCAN = 100
/** Error-dot query size (logs are newest first; the level filter runs after this cut). */
export const ERROR_LOG_LIMIT = 50
/** Error-dot window (`since`, RFC 3339). */
export const ERROR_LOG_WINDOW_MS = 24 * 3_600_000
/** Activity log list size. */
export const LOG_LIMIT = 200
/** Recent sessions window when the agent has no `created_at`. */
export const SESSIONS_FALLBACK_MS = 7 * 86_400_000
/** Owner names for superusers (`GET /api/users` page size). */
export const USERS_LIMIT = 200
/** Rows shown: recent sessions on Activity, error lines on the OSS crash card. */
export const RECENT_SESSIONS_SHOWN = 10
export const CRASH_LINES_SHOWN = 5
/** Catalog: tag chips offered, skill chips per card. */
export const TOP_TAGS = 8
export const CARD_SKILLS = 3
/** `/api/search/users` needs at least this many characters. */
export const USER_SEARCH_MIN = 2
export const USER_SEARCH_MAX = 50
/** Server text shown next to our own copy is cut to this many characters. */
export const SERVER_TEXT_MAX = 200
/** How long an inline success note stays (lives in lib: useCopy shares it). */
export { SAVED_NOTE_MS } from '@/lib/useCopy'
