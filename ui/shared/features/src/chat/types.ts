// Ported from nasiko-ui-lab@1b99bdda common/src/features/chat/types.ts. Local edits: `| undefined` on the optional
// fields (`exactOptionalPropertyTypes` here: the reducer, the turn engine and the views fill them from possibly-absent
// values); Prettier formatting.
/**
 * Chat wire types, from nasiko-cloud-rs at cb3aaf0c:
 * - `ChatSessionView`, `ChatMessage`, `CursorPage` in oss/server/src/chat/models.rs
 * - the HITL DTO from `router/hitl.rs::to_response`
 * - A2A stream frames from oss/types/src/a2a.rs and `a2a_dispatch.rs::normalize_agent_event`
 * Field names are the server's (snake_case); frames are camelCase A2A.
 */

/** Proposed server contract (plan §10.2): who writes each transcript row. Absent at cb3aaf0c. */
export interface TranscriptOwnership {
  user: "client" | "server";
  assistant: "client" | "server";
}

/** `ChatSessionView` (list rows) and `ChatSession` (create/rename responses) share these fields. */
export interface ChatSessionRow {
  session_id: string;
  user_id?: string | undefined;
  agent_id: string | null;
  agent_url?: string | null | undefined;
  title: string;
  created_at: string;
  /** Absent on the create response (`ChatSession` has no updated_at). */
  updated_at?: string | undefined;
  agent_name?: string | null | undefined;
  is_coding_agent?: boolean | undefined;
  last_message?: string | null | undefined;
  message_count?: number | null | undefined;
  trace_count?: number | null | undefined;
  total_tokens?: number | null | undefined;
  latency_p50_ms?: number | null | undefined;
  /** Not sent by cb3aaf0c; see plan §6.1 step 9. */
  transcript?: TranscriptOwnership | undefined;
}

export interface CursorPage<T> {
  data: T[];
  has_more: boolean;
  next_cursor: string | null;
  prev_cursor: string | null;
}

export interface FilePart {
  id: string;
  name: string;
  size?: number | undefined;
  mime?: string | undefined;
}

export interface ChatMessage {
  id: string;
  session_id: string;
  external_turn_id?: string | null | undefined;
  role: "user" | "assistant" | "system";
  content: string;
  file_parts?: FilePart[] | null | undefined;
  has_file_parts?: boolean | undefined;
  timestamp: string;
  input_tokens?: number | null | undefined;
  output_tokens?: number | null | undefined;
  /** chat/models.rs at ea233d20; null unless the agent reported cache usage. */
  cache_read_tokens?: number | null | undefined;
  cache_creation_tokens?: number | null | undefined;
  model?: string | null | undefined;
  duration_ms?: number | null | undefined;
  /** `rust_decimal::Decimal` serialises as a JSON string. */
  cost_usd?: string | number | null | undefined;
  usage_estimated?: boolean | null | undefined;
  trace_id?: string | null | undefined;
  metadata?: Record<string, unknown> | null | undefined;
}

export type HitlKind = "input_required" | "auth_required" | "tool_approval";
type HitlStatus = "pending" | "resolved" | "rejected" | "expired" | "canceled";
type ResumeStatus = "not_started" | "completed" | "failed" | "delivery_outcome_unknown" | "skipped";

export interface HitlOption {
  label: string;
  description?: string | undefined;
}

/** `question` JSON; tool approvals carry `connector_id`/`tool_name`, never arguments (protocol.rs:700-730). */
interface HitlQuestion {
  message?: string | undefined;
  header?: string | undefined;
  options?: HitlOption[] | undefined;
  multi_select?: boolean | undefined;
  allow_custom_input?: boolean | undefined;
  auth_url?: string | undefined;
  provider?: string | undefined;
  expected_input?: string | undefined;
  connector_id?: string | undefined;
  tool_name?: string | undefined;
  [key: string]: unknown;
}

export interface HitlDto {
  id: string;
  kind: HitlKind;
  status: HitlStatus;
  resume_status: ResumeStatus;
  question: HitlQuestion | null;
  human_response: unknown;
  execution: {
    origin: string;
    agent_id: string | null;
    task_id: string | null;
    context_id: string | null;
    chat_session_id: string | null;
    maf_execution_id: string | null;
    maf_step_index: number | null;
  };
  /** Static per kind and always includes `cancel` (hitl.rs:256-262); not filtered by status. */
  allowed_actions: string[];
  expires_at: string;
  created_at: string;
  resolved_at: string | null;
  already_resolved?: boolean | undefined;
  already_canceled?: boolean | undefined;
}

export interface MessagesPage extends CursorPage<ChatMessage> {
  hitl: HitlDto[];
}

/** `usage_meta` data part (router/usage_meta.rs:51-68); token/cost keys only when tokens > 0. */
export interface UsageMeta {
  duration_ms?: number | undefined;
  trace_id?: string | undefined;
  input_tokens?: number | undefined;
  output_tokens?: number | undefined;
  total_tokens?: number | undefined;
  cost_usd?: number | undefined;
  estimated?: boolean | undefined;
  model?: string | undefined;
}

/** The `{type:"hitl", …}` data part a pause emits after the request row is committed. */
export interface HitlFrame {
  id: string;
  kind: HitlKind;
  question: HitlQuestion | null;
  agent?: string | undefined;
  task_id?: string | undefined;
  context_id?: string | undefined;
}

/** What the client saves for an assistant row (`SendMessage` + `MessageUsage`). */
export interface SaveMessageBody {
  role: "user" | "assistant";
  content: string;
  usage?:
    | {
        input_tokens?: number | undefined;
        output_tokens?: number | undefined;
        model?: string | undefined;
        duration_ms?: number | undefined;
        cost_usd?: number | undefined;
        estimated?: boolean | undefined;
        trace_id?: string | undefined;
      }
    | undefined;
}
