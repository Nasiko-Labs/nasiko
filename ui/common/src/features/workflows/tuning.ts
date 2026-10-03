/** Every workflow tuning number (plans/feat-workflows.md). */

/** Lists ask for the first 100 (the React flow's parity ceiling; the server has `search` for when that bites). */
export const LIST_LIMIT = 100
/** `list_all_executions` clamps a page to 50. */
export const RUNS_LIMIT = 50
/** A moving run (queued, running, paused) is re-read this often. */
export const RUN_POLL_MS = 1_500
