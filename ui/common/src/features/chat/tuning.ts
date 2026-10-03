/**
 * Every Chat timing and size limit (plan §8.4). Dev and mock builds may override values through
 * `localStorage['ui-lab:chat-tuning']` (a JSON object) or `?tune=<key>:<ms>` so /qa can shorten
 * the slow paths.
 */

import { DRAIN_CEILING_MS } from './serverContract'

const DEFAULTS = {
  /** Follow new output while the transcript is within this many px of the bottom. */
  FOLLOW_PX: 80,
  /** Add an elapsed timer to the waiting line after this long without a reply. */
  SLOW_MS: 8_000,
  /** Add "Still working…" after this long. */
  LONG_MS: 30_000,
  /** Show the "no update" notice after this long without a frame. The read continues. */
  STREAM_IDLE_MS: 90_000,
  /** Give up waiting for create + title (the server titles synchronously with an LLM call). */
  CREATE_TIMEOUT_MS: 20_000,
  /**
   * An unanswered user message younger than this is "checking", older is "unconfirmed".
   * Must exceed the server's FLOW_TIMEOUT_SECS and ROUTER_AGENT_TIMEOUT_SECS.
   */
  LOST_REPLY_AFTER_MS: 600_000,
  /**
   * How long an answered routed request whose delivery hasn't started (`resume_status: not_started`)
   * may still produce a reply: 5 delivery attempts × the 300 s agent timeout (hitl/mod.rs,
   * cb3aaf0c), plus the orchestrator's turn. Drift-prone: server constants.
   */
  RESUME_DELIVERY_MS: 30 * 60_000,
  /** A reply this young may still be arriving in Tempo. */
  TRACE_FRESH_MS: 60_000,
  /** Agent status refresh while the not-running banner shows. */
  STATUS_POLL_MS: 15_000,
  /** History refresh while a chat is waiting on a request (expiry unlocks it). */
  PENDING_POLL_MS: 30_000,
  /** Unreadable `data:` lines skipped before the turn fails. */
  MAX_BAD_FRAMES: 20,
  /** One SSE event, in characters. */
  MAX_EVENT_CHARS: 1024 * 1024,
  /** One turn's stream, in bytes. */
  MAX_TURN_BYTES: 5 * 1024 * 1024,
  /** Raw frames kept for `?debug=turn`. */
  DEBUG_FRAMES: 200,
  DEBUG_FRAME_CHARS: 4096,
  /** How long "Replaced your saved draft · Undo" is offered after a target choice (v1c DS-T1). */
  UNDO_DRAFT_MS: 10_000,
  /** A direct reply whose save hasn't settled by then still ends, as a reply (v1c §5.8). */
  DIRECT_END_SAVE_CAP_MS: 20_000,
  /** How long "Reply ready in <title>" stays, counted while the tab is visible (v1c §5.8, DS4). */
  REPLY_READY_MS: 60_000,
  /** The rail re-reads the clock this often, so date groups and "3 min ago" move (midnight included). */
  RAIL_CLOCK_MS: 60_000,
  /** A recorded chat's relative call times re-read the clock this often. */
  RECORDED_CLOCK_MS: 15_000,
  /** Characters of one saved step's tool result, or one recorded call's detail section, before it is cut. */
  SAVED_DETAIL_MAX: 2000,
  /** Largest draft kept in localStorage. */
  DRAFT_MAX_CHARS: 20_000,
  /** Largest message a user can send. */
  MESSAGE_MAX_CHARS: 100_000,
  /** Live turns per tab before a new routed send is refused (HTTP/1.1: 6 connections per origin, G-10). */
  MAX_LIVE_TURNS: 3,
  /** Characters of an agent's excerpt in expanded Activity (NC-4). */
  ACTIVITY_EXCERPT_CHARS: 280,
  /** See serverContract.ts; here so /qa can shorten it. */
  DRAIN_CEILING_MS,
  /** After a routed turn ends, history re-checks for its saved reply: this many, backing off from ROUTED_RECHECK_MS. */
  ROUTED_RECHECKS: 3,
  ROUTED_RECHECK_MS: 1_500,
  /** A routed resume whose chat lock is held (release in flight, or another tab) retries this often, this many times. */
  RESUME_LOCK_RETRIES: 60,
  RESUME_LOCK_RETRY_MS: 500,
  /** A routed reconnect with no frame by then is an expired buffer (the server keeps it open, silent, for ~1 h). */
  RESUME_FIRST_FRAME_MS: 90_000,
  /** Steps kept per turn; a stream with more is flooding (tool-call pairing is per step). */
  MAX_STEPS: 500,
  /** Turn ends kept per registry (NE-4); oldest first out. */
  TURN_ENDS_MAX: 200,
  /** Request ids the Waiting index keeps (v1c EN10); oldest first out. */
  HITL_INDEX_MAX: 2_000,
  /** Activity lines kept per turn (sub-agent text is chunked). */
  MAX_ACTIVITY: 200,
  /** Characters of an agent's streamed sub_content kept per turn, and of its one-line sub_status. */
  AGENT_NOTE_MAX_CHARS: 4_000,
  ACTIVITY_NOTE_CHARS: 200,
  /** Flows requests in flight, and flows kept in the cache (routed attribution, E-A6, NE-14). */
  FLOWS_IN_FLIGHT: 4,
  FLOWS_CACHE_MAX: 500,
  /** Chat lookup past the rail (EN-6): pages and page size. */
  LOOKUP_PAGES: 20,
  LOOKUP_PAGE_SIZE: 100,
}

export type ChatTuning = typeof DEFAULTS

const valid = (k: string, v: unknown): v is number =>
  k in DEFAULTS && typeof v === 'number' && Number.isFinite(v) && v >= 0

/** `?tune=<key>:<ms>` (repeatable), e.g. `?tune=LOST_REPLY_AFTER_MS:5000` (v1b NX-8). */
export function tuneParams(search: string): Partial<ChatTuning> {
  const out: Partial<ChatTuning> = {}
  for (const pair of new URLSearchParams(search).getAll('tune')) {
    const [k = '', v = ''] = pair.split(':')
    const n = Number(v)
    if (v.trim() && valid(k, n)) (out as Record<string, number>)[k] = n
  }
  return out
}

/** Dev and mock builds only: `import.meta.env.DEV` is false in production, so the build drops this. */
function overrides(): Partial<ChatTuning> {
  if (!import.meta.env.DEV) return {}
  const out: Partial<ChatTuning> = {}
  try {
    const raw = globalThis.localStorage?.getItem('ui-lab:chat-tuning')
    const parsed: unknown = raw ? JSON.parse(raw) : null
    if (parsed && typeof parsed === 'object')
      for (const [k, v] of Object.entries(parsed))
        if (valid(k, v)) (out as Record<string, number>)[k] = v
  } catch {
    // Blocked storage or bad JSON: no stored overrides.
  }
  return { ...out, ...tuneParams(globalThis.location?.search ?? '') }
}

export const tuning: ChatTuning = { ...DEFAULTS, ...overrides() }
