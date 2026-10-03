/** Every tuning number of the router page (plan §7). */

/** At most this many `GET /api/agents/{id}/llm-config` reads in flight (the page's own limiter, eng #4). */
export const ROUTING_READ_CONCURRENCY = 4
// R-L8, R-L13: per-agent reads fan out (no batch read) and the router caches configs this long anyway.
export const ROUTING_STALE_MS = 30_000
// R-L8: server default LLM_CONFIG_CACHE_TTL; not exposed, so a server set differently makes this copy wrong.
export const CONFIG_CACHE_SECONDS = 30
/** Lists (configs, secrets, catalog, custom providers, registry) count as fresh this long before a focus refetch. */
export const LIST_STALE_MS = 10_000
/** Agents on your default fold into one summary row once there are at least this many (fewer read fine as rows). */
export const DEFAULT_GROUP_MIN = 3
/** A name search appears above Your agents from this many agents. */
export const AGENT_SEARCH_MIN = 20
/** A new budget's alert marks, in percent of the limit (R-L10 default). */
export const DEFAULT_THRESHOLDS = [50, 80, 100] as const
/** Spend is a 30-day aggregate: re-read after this long, never on focus. */
export const SPEND_STALE_MS = 5 * 60_000
/** Router-metered spend window (E1), in days. */
export const SPEND_DAYS = 30
/** One by-agent request this large (the server sorts with no tie-breaker, so offset paging could repeat rows). */
export const SPEND_LIMIT = 1000
