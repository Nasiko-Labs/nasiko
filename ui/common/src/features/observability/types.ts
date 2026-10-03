/**
 * Wire types for the observability endpoints, taken from the generated OpenAPI schema
 * (`npm run gen:api` against nasiko-server @ cb3aaf0c). The MSW handlers return values
 * that `satisfies` these, so mock and server can't drift silently.
 *
 * Contract facts that shape the UI (oss/server/src/observability, cb3aaf0c):
 * - `session/list` takes only start_time/limit(1–100)/offset, ordered created_at DESC,
 *   rows come from chat_sessions; `agent_id` is the raw agent name ("" when unresolved).
 * - `TraceDetail.spans` is a TREE of root nodes; `span_lookup` is the flat map keyed by
 *   the base64 `id`. `span_id` is raw hex and is what `GET /span/{trace}/{span}` matches.
 * - `span_kind` is the OTel kind (internal/server/client/…), not planner/tool/llm.
 * - `SpanNode` has no cost; per-span cost is only on `SpanDetail.cost_summary`.
 * - Errors are plain text. 503 = observability not configured, 500/502 = trace store
 *   error, 404 = not found or not visible (access denial is a 404 on purpose).
 */
import type { components } from '@/lib/api/schema.gen'

type S = components['schemas']

export type SessionListResponse = S['SessionListResponse']
export type SessionSummary = S['SessionSummary']
export type SessionDetailResponse = S['SessionDetailResponse']
export type SessionDetail = S['SessionDetail']
export type TraceEntry = S['TraceEntry']
export type TraceDetailResponse = S['TraceDetailResponse']
export type TraceDetail = S['TraceDetail']
export type SpanNode = S['SpanNode']
/**
 * The spec types `parsed_value` as an object, but the server sends whatever the content parses to as JSON: an array
 * for GenAI messages (service.rs get_span_details at ea233d20; recommendation C-4). Attributes arrive un-flattened on
 * dots (`unflatten_attrs`); read them with `flattenAttributes` (spans.ts).
 */
export type ContentField = Omit<S['ContentField'], 'parsed_value'> & { parsed_value?: unknown }
export type SpanDetail = Omit<S['SpanDetail'], 'input' | 'output'> & {
  input: ContentField
  output: ContentField
}
export interface SpanDetailResponse {
  data: { span: SpanDetail }
}
export type LogLine = S['LogLine']
