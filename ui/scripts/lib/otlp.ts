/**
 * OTLP/HTTP JSON for the seed's traces (plans/feat-live-contract.md §6): the observability mock's span trees
 * (src/mocks/spanBuilder.ts), written the way nasiko-server at ea233d20 reads them back from Tempo:
 * - the agent is the resource's `service.name` (the materializer and spend timeseries group by it);
 * - `session.id` is a span attribute (TraceQL `{span.session.id="…"}`, provider.rs `session_query`), on the root;
 * - usage comes from `gen_ai.usage.*` on the LLM spans (observability/src/types.rs `extract_usage_attrs`), with
 *   `nasiko.usage.prompt_convention` = "exclusive" so cached tokens aren't subtracted from input again;
 * - the model and provider from `gen_ai.request.model` and `gen_ai.provider.name`/`gen_ai.system`;
 * - root content from `gen_ai.input.messages`/`gen_ai.output.messages`, span content from `input.value`/`output.value`;
 * - a `coding_agent.turn` root carries no usage of its own: the server sums the whole trace onto it.
 * Cost is never written: the server prices usage from its own `model_pricing` table.
 * The shape follows the server's own emitter (server/src/coding_agent_otlp.rs `trace_payload`).
 */
import {
  generateSpans,
  observabilityData,
  spanAttributes,
  type GenSpan,
  type MockSession,
} from '../../common/src/mocks/spanBuilder.ts'
import type { Seed, SeedTrace } from '../../common/src/mocks/seed.ts'

/**
 * Days of seed traces sent to Tempo, counted back from the anchor: inside Tempo's 7-day search limit
 * (provider.rs `clamp_tempo_range`, 168 h), with a day to spare for the recording itself.
 */
export const TRACE_WINDOW_DAYS = 6

type AnyValue =
  { stringValue: string } | { intValue: string } | { doubleValue: number } | { boolValue: boolean }
export interface OtlpAttribute {
  key: string
  value: AnyValue
}
export interface OtlpSpan {
  traceId: string
  spanId: string
  parentSpanId: string
  name: string
  kind: number
  startTimeUnixNano: string
  endTimeUnixNano: string
  attributes: OtlpAttribute[]
  status: { code: number; message?: string }
}
export interface OtlpPayload {
  resourceSpans: {
    resource: { attributes: OtlpAttribute[] }
    scopeSpans: { scope: { name: string }; spans: OtlpSpan[] }[]
  }[]
}

/** OTLP SpanKind: 1 internal, 2 server, 3 client. */
const KIND = { internal: 1, server: 2, client: 3 } as const
/** OTLP StatusCode: 0 unset, 1 ok, 2 error. */
const STATUS = { UNSET: 0, OK: 1, ERROR: 2 } as const

export function attr(key: string, v: unknown): OtlpAttribute | null {
  if (v === null || v === undefined || v === '') return null
  if (typeof v === 'boolean') return { key, value: { boolValue: v } }
  if (typeof v === 'number')
    return { key, value: Number.isInteger(v) ? { intValue: String(v) } : { doubleValue: v } }
  return { key, value: { stringValue: typeof v === 'string' ? v : JSON.stringify(v) } }
}
const attrs = (o: Record<string, unknown>) =>
  Object.entries(o)
    .map(([k, v]) => attr(k, v))
    .filter((a): a is OtlpAttribute => a !== null)
const nanos = (ms: number) => `${BigInt(Math.round(ms)) * 1_000_000n}`

/** One seed trace's spans, as OTLP spans. */
export function otlpSpans(t: SeedTrace, session: MockSession, gen: readonly GenSpan[]): OtlpSpan[] {
  return gen.map((s) => ({
    traceId: t.trace_id,
    spanId: s.hex,
    parentSpanId: s.parentHex ?? '',
    name: s.name,
    kind: KIND[s.kind],
    startTimeUnixNano: nanos(s.start),
    endTimeUnixNano: nanos(s.end),
    attributes: attrs(spanAttributes(s, session)),
    status:
      s.status === 'ERROR'
        ? { code: STATUS.ERROR, message: s.statusMessage }
        : { code: STATUS[s.status] },
  }))
}

/** Seed traces whose chat session started at or after `since`, newest sessions first, grouped per agent. */
export function seedTraceSessions(seed: Seed, since: number): MockSession[] {
  return observabilityData(seed).sessions.filter((s) => s.created >= since)
}

/**
 * OTLP payloads for every trace of the given sessions, one resource per agent, at most `maxSpans` spans per payload
 * (Tempo's default max request size is 4 MB).
 */
export function otlpPayloads(
  seed: Seed,
  sessions: readonly MockSession[],
  maxSpans = 2_000,
): OtlpPayload[] {
  const out: OtlpPayload[] = []
  let byAgent = new Map<string, OtlpSpan[]>()
  let count = 0
  const flush = () => {
    if (!count) return
    out.push({
      resourceSpans: [...byAgent].map(([agent, spans]) => ({
        resource: { attributes: attrs({ 'service.name': agent, 'agent.id': agent }) },
        scopeSpans: [{ scope: { name: 'nasiko-ui-lab-seed' }, spans }],
      })),
    })
    byAgent = new Map()
    count = 0
  }
  for (const s of sessions) {
    for (const t of s.traces) {
      const spans = otlpSpans(t, s, generateSpans(seed, t.trace_id))
      if (count + spans.length > maxSpans) flush()
      byAgent.set(s.agent.name, [...(byAgent.get(s.agent.name) ?? []), ...spans])
      count += spans.length
    }
  }
  flush()
  return out
}
