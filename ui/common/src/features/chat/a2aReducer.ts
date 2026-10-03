/**
 * The one place that knows A2A stream shapes (plan §3.2, S6).
 *
 *   SSE data line ──JSON──► classifyFrame ──► TypedEvent[] ──► applyEvent ──► TurnState
 *
 * Shapes handled, bare or under `result.` (JSON-RPC):
 * - 1.0 `statusUpdate`, `artifactUpdate`, `task`, `message`
 * - 0.3 `result.kind` = status-update | artifact-update | task | message (`final` means lastChunk);
 *   the server normalises only the first two (a2a_dispatch.rs:2125-2181)
 * - JSON-RPC `{error:{code,message}}` and the `event: error` frame `{error}`
 * Task states arrive as `TASK_STATE_*` (Nasiko) or lowercase/hyphenated (some agents).
 * Nasiko's own data parts (trace_meta, usage_meta, hitl, steps) ride in WORKING statuses.
 *
 * Pure: no DOM, no fetch, no clock.
 */
import { toNumber } from './normalize'
import { foldAgentName } from './agentName'
import { isTruncationMarker } from './serverContract'
import { tuning } from './tuning'
import type { HitlFrame, HitlKind, UsageMeta } from './types'

export type TaskState =
  | 'submitted'
  | 'working'
  | 'completed'
  | 'failed'
  | 'canceled'
  | 'rejected'
  | 'input_required'
  | 'auth_required'
  | 'unknown'

export interface Step {
  /** Stable key: a tool_call and the tool_result paired with it share it. */
  key: string
  kind: 'agent' | 'tool' | 'policy'
  name: string
  status: 'running' | 'ok' | 'error'
  detail?: string
  durationMs?: number
  /** `via_agent`: set when a nested agent relayed this step (not a top-level call, EN-7). */
  via?: string
}

/**
 * Pairing data per call step, by step key: the server's `turn` (several calls can share one,
 * G-1) and the invocation key (agent + turn + per-turn sequence + via). Kept off `Step` so the
 * v1a step shape is unchanged.
 */
interface CallMeta {
  turn: string
  invocationKey: string
}

export type TypedEvent =
  | { type: 'status'; state: TaskState; text: string }
  | { type: 'artifact'; artifactId: string; text: string; append: boolean; lastChunk: boolean }
  | { type: 'message'; text: string }
  | { type: 'task'; state: TaskState; artifacts: { id: string; text: string }[]; text: string }
  | { type: 'trace'; traceId: string }
  | { type: 'usage'; usage: UsageMeta }
  | { type: 'hitl'; request: HitlFrame }
  | { type: 'step'; step: Step; merge: boolean; pair?: 'call' | 'result'; turn?: string }
  | { type: 'awaiting'; agent?: string; message?: string }
  | { type: 'agentNote'; agent: string; field: 'status' | 'content'; text: string }
  | { type: 'activity'; text: string }
  | { type: 'error'; message: string; code?: number }

export interface TurnState {
  /** Reply artifacts in arrival order; `append:false` replaces one by id. */
  artifacts: { id: string; text: string }[]
  /** A complete reply from a `message` or `task` frame, used when no artifact text exists. */
  messageText: string
  /** Text carried on a terminal COMPLETED status, used as a last resort. */
  statusText: string
  steps: Step[]
  activity: string[]
  traceId: string | null
  usage: UsageMeta | null
  request: HitlFrame | null
  taskState: TaskState | null
  error: { message: string; code?: number } | null
  badFrames: number
  /** `awaiting_human{agent,message}` before a routed pause's `hitl` frame (§2.7). */
  awaiting: { agent?: string; message?: string } | null
  /** The continuation buffer's truncation marker arrived (§5.3). */
  truncated: boolean
  calls: Record<string, CallMeta>
  /**
   * Per agent (by folded name: sub_* frames carry the raw name, tool_call the display form): its
   * latest `sub_status` and `sub_content` (routed Activity, ND-4, NC-4). Plain text, never a reply.
   */
  agentNotes: Record<string, { status?: string; content?: string }>
  /**
   * How many artifacts existed when the first `trace_meta` arrived. A routed reconnect replays the
   * sub-agent's own resumed stream before the orchestrator's new turn, which starts with
   * `trace_meta` (hitl/mod.rs deliver → trigger_new_orchestrator_turn): only later artifacts are
   * OpenRuntime's reply.
   */
  replyArtifactsFrom: number | null
}

export const emptyTurn = (): TurnState => ({
  artifacts: [],
  messageText: '',
  statusText: '',
  steps: [],
  activity: [],
  traceId: null,
  usage: null,
  request: null,
  taskState: null,
  error: null,
  badFrames: 0,
  awaiting: null,
  truncated: false,
  calls: {},
  agentNotes: {},
  replyArtifactsFrom: null,
})

/**
 * A routed reply is the orchestrator's artifact text only; `sub_content`, `sub_status` and status
 * text are activity (§5.3, matching the server's `full_reply`, a2a_dispatch.rs:807).
 */
export function routedReplyText(s: TurnState, resume = false): string {
  // A resume's replay opens with the sub-agent's own stream: nothing is the reply until the
  // orchestrator's turn announces itself with trace_meta.
  if (resume && s.replyArtifactsFrom === null) return ''
  return s.artifacts
    .slice(s.replyArtifactsFrom ?? 0)
    .map((a) => a.text)
    .join('')
}

/** An agent's notes, whichever spelling of its name a frame used. */
export const notesFor = (s: Pick<TurnState, 'agentNotes'>, agent: string) =>
  s.agentNotes[foldAgentName(agent)]

/** The reply to show and save. */
export function replyText(s: TurnState): string {
  const fromArtifacts = s.artifacts.map((a) => a.text).join('')
  if (fromArtifacts.trim()) return fromArtifacts
  if (s.messageText.trim()) return s.messageText
  return s.statusText
}

const STATE_ALIASES: Record<string, TaskState> = {
  submitted: 'submitted',
  working: 'working',
  completed: 'completed',
  failed: 'failed',
  canceled: 'canceled',
  cancelled: 'canceled',
  rejected: 'rejected',
  input_required: 'input_required',
  auth_required: 'auth_required',
}

export function normalizeState(raw: unknown): TaskState {
  if (typeof raw !== 'string') return 'unknown'
  const k = raw
    .toLowerCase()
    .replace(/^task_state_/, '')
    .replace(/-/g, '_')
  return STATE_ALIASES[k] ?? 'unknown'
}

type Json = Record<string, unknown>
const isObj = (v: unknown): v is Json => typeof v === 'object' && v !== null && !Array.isArray(v)
const str = (v: unknown): string | undefined => (typeof v === 'string' ? v : undefined)

interface Part {
  text?: string
  data?: Json
}

/** 1.0 parts are `{text}`/`{data}`; 0.3 parts add `kind: 'text' | 'data'`. */
function readParts(raw: unknown): Part[] {
  if (!Array.isArray(raw)) return []
  const out: Part[] = []
  for (const p of raw) {
    if (!isObj(p)) continue
    const text = str(p.text)
    if (text !== undefined) out.push({ text })
    else if (isObj(p.data)) out.push({ data: p.data })
    else if (isObj(p.root)) out.push(...readParts([p.root]))
  }
  return out
}

const textOf = (parts: Part[]) => parts.map((p) => p.text ?? '').join('')

const HITL_KINDS: readonly HitlKind[] = ['input_required', 'auth_required', 'tool_approval']

/** Nasiko data parts → events. Unknown types are ignored. */
function dataEvents(d: Json): TypedEvent[] {
  const type = str(d.type)
  switch (type) {
    case 'trace_meta': {
      const id = str(d.trace_id)
      return id ? [{ type: 'trace', traceId: id }] : []
    }
    case 'usage_meta': {
      const usage: UsageMeta = {}
      const n = (k: string) => toNumber(d[k])
      if (n('duration_ms') !== null) usage.duration_ms = n('duration_ms') as number
      if (str(d.trace_id)) usage.trace_id = str(d.trace_id)
      if (n('input_tokens') !== null) usage.input_tokens = n('input_tokens') as number
      if (n('output_tokens') !== null) usage.output_tokens = n('output_tokens') as number
      if (n('total_tokens') !== null) usage.total_tokens = n('total_tokens') as number
      if (n('cost_usd') !== null) usage.cost_usd = n('cost_usd') as number
      if (typeof d.estimated === 'boolean') usage.estimated = d.estimated
      if (str(d.model)) usage.model = str(d.model)
      return [{ type: 'usage', usage }]
    }
    case 'hitl': {
      const id = str(d.id)
      const kind = str(d.kind) as HitlKind | undefined
      if (!id || !kind || !HITL_KINDS.includes(kind)) return []
      return [
        {
          type: 'hitl',
          request: {
            id,
            kind,
            question: isObj(d.question) ? d.question : null,
            agent: str(d.agent),
            task_id: str(d.task_id),
            context_id: str(d.context_id),
          },
        },
      ]
    }
    case 'thinking': {
      const c = str(d.content)
      return c ? [{ type: 'activity', text: c }] : []
    }
    case 'sub_status':
    case 'sub_content': {
      const c = str(d.content) ?? str(d.message) ?? str(d.status)
      if (!c) return []
      const agent = str(d.agent)
      const note: TypedEvent[] = agent
        ? [
            {
              type: 'agentNote',
              agent,
              field: type === 'sub_status' ? 'status' : 'content',
              text: c,
            },
          ]
        : []
      return [{ type: 'activity', text: c }, ...note]
    }
    case 'tool_call': {
      const agent = str(d.agent) ?? str(d.tool) ?? 'tool'
      const turn = String(d.turn ?? '')
      return [
        {
          type: 'step',
          merge: false,
          pair: 'call',
          turn,
          step: {
            key: '',
            via: str(d.via_agent),
            kind: str(d.agent) ? 'agent' : 'tool',
            name: agent,
            status: 'running',
            detail: str(d.message),
          },
        },
      ]
    }
    case 'tool_result': {
      const agent = str(d.agent) ?? str(d.tool) ?? 'tool'
      const turn = String(d.turn ?? '')
      const ok = d.success !== false
      return [
        {
          type: 'step',
          merge: true,
          pair: 'result',
          turn,
          step: {
            key: '',
            via: str(d.via_agent),
            kind: str(d.agent) ? 'agent' : 'tool',
            name: agent,
            status: ok ? 'ok' : 'error',
            durationMs: toNumber(d.duration_ms) ?? undefined,
            detail: str(d.result),
          },
        },
      ]
    }
    case 'awaiting_human':
      return [{ type: 'awaiting', agent: str(d.agent), message: str(d.message) }]
    case 'policy_rejected': {
      const name = str(d.agent) ?? str(d.tool) ?? 'policy'
      return [
        {
          type: 'step',
          merge: false,
          step: {
            key: `policy#${name}#${str(d.reason) ?? ''}`,
            kind: 'policy',
            name,
            status: 'error',
            detail: str(d.reason) ?? str(d.message),
          },
        },
      ]
    }
    default:
      return []
  }
}

function statusEvents(status: unknown, text: string, parts: Part[]): TypedEvent[] {
  const state = normalizeState(isObj(status) ? status.state : undefined)
  const events: TypedEvent[] = []
  for (const p of parts) if (p.data) events.push(...dataEvents(p.data))
  events.push({ type: 'status', state, text })
  return events
}

function statusFrame(u: Json): TypedEvent[] {
  const status = u.status
  const msg = isObj(status) ? status.message : undefined
  const parts = isObj(msg) ? readParts(msg.parts) : []
  return statusEvents(status, textOf(parts), parts)
}

function artifactFrame(u: Json): TypedEvent[] {
  const art = isObj(u.artifact) ? u.artifact : {}
  const parts = readParts(art.parts)
  const events: TypedEvent[] = []
  for (const p of parts) if (p.data) events.push(...dataEvents(p.data))
  events.push({
    type: 'artifact',
    artifactId: str(art.artifactId) ?? str(art.artifact_id) ?? 'default',
    text: textOf(parts),
    // A2A: append defaults to false (replace); lastChunk may be spelled `final` in 0.3.
    append: u.append === true,
    lastChunk: u.lastChunk === true || u.final === true,
  })
  return events
}

function taskFrame(t: Json): TypedEvent[] {
  const artifacts = Array.isArray(t.artifacts)
    ? t.artifacts.filter(isObj).map((a, i) => ({
        id: str(a.artifactId) ?? str(a.artifact_id) ?? `task-${i}`,
        text: textOf(readParts(a.parts)),
      }))
    : []
  const status = t.status
  const msg = isObj(status) ? status.message : undefined
  const parts = isObj(msg) ? readParts(msg.parts) : []
  const events: TypedEvent[] = []
  for (const p of parts) if (p.data) events.push(...dataEvents(p.data))
  events.push({
    type: 'task',
    state: normalizeState(isObj(status) ? status.state : undefined),
    artifacts,
    text: textOf(parts),
  })
  return events
}

function messageFrame(m: Json): TypedEvent[] {
  const parts = readParts(m.parts)
  const events: TypedEvent[] = []
  for (const p of parts) if (p.data) events.push(...dataEvents(p.data))
  const text = textOf(parts)
  if (text) events.push({ type: 'message', text })
  return events
}

function errorEvent(e: unknown): TypedEvent {
  if (isObj(e))
    return {
      type: 'error',
      message: str(e.message) ?? 'error',
      code: typeof e.code === 'number' ? e.code : undefined,
    }
  return { type: 'error', message: typeof e === 'string' ? e : 'error' }
}

/** One parsed `data:` payload → events. Unrecognised shapes → []. */
export function classifyFrame(frame: unknown): TypedEvent[] {
  if (!isObj(frame)) return []
  if ('error' in frame && frame.error != null) return [errorEvent(frame.error)]
  const body = isObj(frame.result) ? frame.result : frame
  if (isObj(body.statusUpdate)) return statusFrame(body.statusUpdate)
  if (isObj(body.artifactUpdate)) return artifactFrame(body.artifactUpdate)
  if (isObj(body.task)) return taskFrame(body.task)
  if (isObj(body.message)) return messageFrame(body.message)
  // quirk: §10.8 — 0.3 `kind` frames pass through normalize_agent_event unnormalised.
  switch (str(body.kind)) {
    case 'status-update':
      return statusFrame(body)
    case 'artifact-update':
      return artifactFrame(body)
    case 'task':
      return taskFrame(body)
    case 'message':
      return messageFrame(body)
    default:
      return []
  }
}

/** Fold one event into the turn. Returns a new object; the input is never mutated. */
export function applyEvent(s: TurnState, e: TypedEvent): TurnState {
  switch (e.type) {
    case 'artifact': {
      const i = s.artifacts.findIndex((a) => a.id === e.artifactId)
      const artifacts = [...s.artifacts]
      if (i < 0) artifacts.push({ id: e.artifactId, text: e.text })
      else artifacts[i] = { id: e.artifactId, text: e.append ? artifacts[i].text + e.text : e.text }
      return { ...s, artifacts }
    }
    case 'status': {
      const next: TurnState = { ...s, taskState: e.state }
      if (e.state === 'working') {
        if (isTruncationMarker(e.text)) next.truncated = true
        else if (e.text.trim()) next.activity = [...s.activity, e.text].slice(-tuning.MAX_ACTIVITY)
      } else if (e.state === 'completed') {
        if (e.text.trim() && e.text.length >= s.statusText.length) next.statusText = e.text
      } else if (e.state === 'failed' || e.state === 'rejected' || e.state === 'canceled') {
        next.error = { message: e.text.trim() || e.state }
      }
      return next
    }
    case 'message':
      return { ...s, messageText: e.text }
    case 'task': {
      const next: TurnState = { ...s, taskState: e.state }
      if (e.artifacts.some((a) => a.text)) next.artifacts = e.artifacts
      else if (e.text.trim() && e.state === 'completed') next.messageText = e.text
      if ((e.state === 'failed' || e.state === 'rejected') && !s.error)
        next.error = { message: e.text.trim() || e.state }
      return next
    }
    case 'trace': {
      // The reply boundary is the first trace_meta, even if a usage_meta already supplied a trace id.
      const from = s.replyArtifactsFrom ?? s.artifacts.length
      return s.traceId
        ? { ...s, replyArtifactsFrom: from }
        : { ...s, traceId: e.traceId, replyArtifactsFrom: from }
    }
    case 'usage':
      return { ...s, usage: e.usage, traceId: s.traceId ?? e.usage.trace_id ?? null }
    case 'hitl':
      return { ...s, request: e.request }
    case 'step': {
      if (e.pair) return applyPaired(s, e.step, e.pair, e.turn ?? '')
      const i = s.steps.findIndex((x) => x.key === e.step.key)
      if (i < 0)
        return s.steps.length >= tuning.MAX_STEPS ? s : { ...s, steps: [...s.steps, e.step] }
      const steps = [...s.steps]
      steps[i] = e.merge
        ? { ...steps[i], ...e.step, detail: e.step.detail ?? steps[i].detail }
        : e.step
      return { ...s, steps }
    }
    case 'activity':
      return { ...s, activity: [...s.activity, e.text].slice(-tuning.MAX_ACTIVITY) }
    case 'awaiting':
      return { ...s, awaiting: { agent: e.agent, message: e.message } }
    case 'agentNote': {
      const key = foldAgentName(e.agent)
      const prev = s.agentNotes[key] ?? {}
      // Content accumulates (a sub-agent streams it); a status replaces the last one.
      const next =
        e.field === 'status'
          ? { ...prev, status: e.text }
          : {
              ...prev,
              content: `${prev.content ?? ''}${e.text}`.slice(-tuning.AGENT_NOTE_MAX_CHARS),
            }
      return { ...s, agentNotes: { ...s.agentNotes, [key]: next } }
    }
    case 'error':
      return { ...s, error: { message: e.message, code: e.code } }
  }
}

/**
 * tool_call / tool_result pairing (G-1): the server runs a turn's calls one after another under
 * one `turn`, so each result closes the oldest open call of the same agent, turn and via (FIFO).
 * The first call keeps v1a's `agent#turn` key; later ones add their sequence.
 */
function applyPaired(s: TurnState, step: Step, pair: 'call' | 'result', turn: string): TurnState {
  const same = (x: Step) =>
    x.name === step.name && s.calls[x.key]?.turn === turn && (x.via ?? '') === (step.via ?? '')
  const add = (): TurnState => {
    // A flood of calls or unpaired results: stop tracking them rather than freeze the tab.
    if (s.steps.length >= tuning.MAX_STEPS) return s
    const seq = s.steps.filter(same).length
    const base = `${step.name}#${turn}${step.via ? `#via:${step.via}` : ''}`
    const key = seq ? `${base}#${seq}` : base
    return {
      ...s,
      steps: [...s.steps, { ...step, key }],
      calls: {
        ...s.calls,
        [key]: { turn, invocationKey: `${step.name}#${turn}#${seq}#${step.via ?? ''}` },
      },
    }
  }
  if (pair === 'call') return add()
  const i = s.steps.findIndex((x) => x.status === 'running' && same(x))
  if (i < 0) return add()
  const steps = [...s.steps]
  const open = steps[i]
  steps[i] = {
    ...open,
    status: step.status,
    durationMs: step.durationMs ?? open.durationMs,
    detail: step.detail ?? open.detail,
  }
  return { ...s, steps }
}

/** Parse one SSE event (name + data) and fold it in. Unparseable data counts as a bad frame. */
export function reduceSseEvent(
  s: TurnState,
  ev: { event: string; data: string },
): { state: TurnState; bad: boolean } {
  if (ev.data === '[DONE]') return { state: s, bad: false }
  let parsed: unknown
  try {
    parsed = JSON.parse(ev.data)
  } catch {
    return { state: { ...s, badFrames: s.badFrames + 1 }, bad: true }
  }
  // `event: error` carries `{error: "..."}` (the HITL persist-failure frame, a2a_dispatch.rs:2052).
  const events =
    ev.event === 'error'
      ? [errorEvent(isObj(parsed) ? (parsed.error ?? parsed) : parsed)]
      : classifyFrame(parsed)
  return { state: events.reduce(applyEvent, s), bad: false }
}
