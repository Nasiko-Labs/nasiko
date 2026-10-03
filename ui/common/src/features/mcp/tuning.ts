/** Every MCP tuning number (plans/feat-mcp.md). */

/** Lists and a building server's page refetch this often while an upload builds (legacy: a 5 s timer). */
export const BUILD_POLL_MS = 5_000
/** How often an open OAuth popup is checked for closing (legacy: 500 ms). */
export const POPUP_POLL_MS = 500
export const POPUP_FEATURES = 'width=600,height=720'
/** Per-connector tool reads on the agent's MCP tab, at most this many at once. */
export const TOOL_READ_CONCURRENCY = 4
/** Build log lines read (legacy detail page: 500). */
export const LOG_TAIL = 500
/** The share-target search needs this many characters (legacy). */
export const SHARE_SEARCH_MIN = 2
/** A server's tool list gets a filter from this many tools. */
export const TOOL_FILTER_FROM = 10
export const LIST_STALE_MS = 30_000
