/**
 * Server facts the routed path depends on, in one place (plan §5.3, §5.6, NC-5), verified at
 * nasiko-cloud-rs cb3aaf0c. Each is checked against the captured `live-routed.json` fixture; a
 * server CI contract test is requested in the v1b recommendations.
 */

/** `TRUNCATION_MARKER` text prefix: a `working` status the continuation buffer appends when full (`hitl/continuation.rs:46`). */
export const TRUNCATION_PREFIX = '[replay truncated:'

/** The marker's exact bytes (`hitl/continuation.rs:46`): wrapped in `result`, lowercase state, no task or context id. */
export const TRUNCATION_MARKER =
  '{"result":{"statusUpdate":{"status":{"state":"working","message":{"parts":[{"text":"[replay truncated: this resume produced more events than can be buffered; some output was dropped]"}]}}}}}'

/** The title the server gives a harness session it records (`observability/service.rs` `ensure_session`). */
export const CODING_SESSION_TITLE = 'Coding session'

/** The 429 body `rate_limit.rs:59` sends (plain text; 30 requests a minute per user, reconnects included). */
export const RATE_LIMIT_BODY = 'rate limit exceeded, try again shortly'

// The continuation buffer caps (`hitl/continuation.rs`) are 500 events, then 1 MiB; past them the
// server appends TRUNCATION_MARKER. The client only needs the marker.

/**
 * How long a routed first send keeps draining an over-limit stream before it gives up and
 * reloads the saved reply (§5.3). Drift-prone: the real bound is the server-only
 * `NASIKO_FLOW_TIMEOUT_SECS` (default 120 s, `flow/src/guard.rs:32`).
 */
// scaffolding: remove when the server routed reply tap ships (rec 1)
export const DRAIN_CEILING_MS = 15 * 60_000

export type PolicyLimit = 'depth' | 'cycle' | 'fanOut' | 'tokens' | 'timeout' | 'guard'

/** `FlowRejection`'s Display prefixes (`flow/src/guard.rs:74-100`) → the limit hit. */
const POLICY_PREFIXES: readonly [string, PolicyLimit][] = [
  ['max call depth exceeded', 'depth'],
  ['cycle detected', 'cycle'],
  ['max fan-out exceeded', 'fanOut'],
  ['flow token budget exhausted', 'tokens'],
  ['flow timeout', 'timeout'],
  ['flow guard unavailable', 'guard'],
]

/** The env var an admin raises for each limit (`guard.rs:26-35`); cycle and guard have none. */
export const POLICY_ENV: Partial<Record<PolicyLimit, string>> = {
  depth: 'NASIKO_FLOW_MAX_DEPTH',
  fanOut: 'NASIKO_FLOW_MAX_FAN_OUT',
  tokens: 'NASIKO_FLOW_MAX_TOKENS',
  timeout: 'NASIKO_FLOW_TIMEOUT_SECS',
}

export function policyLimit(reason: string | undefined): PolicyLimit | null {
  const r = (reason ?? '').trim().toLowerCase()
  return POLICY_PREFIXES.find(([p]) => r.startsWith(p))?.[1] ?? null
}

export const isTruncationMarker = (text: string) => text.trimStart().startsWith(TRUNCATION_PREFIX)
