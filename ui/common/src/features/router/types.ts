/**
 * Wire types for the LLM router page, checked against nasiko-server @ cb3aaf0c.
 * - Configs: `server/src/llm_configs.rs` `CONFIG_JSON` (the OpenAPI response is an untyped McpEnvelope).
 * - Agent routing: `server/src/agents/llm_config.rs` `LlmConfigResponse` / `AttachLlmConfigRequest`.
 * - Custom providers: `server/src/llm_router/custom_providers.rs` `ProviderView`, `CreateRequest`, `UpdateRequest`,
 *   `TestRequest` (not in OpenAPI, so typed by hand); `kind`/`api_version` from @ 4d57453c (`llm-router` `dialect.rs`
 *   `KIND_*`). Older servers send no `kind`: read it as `openai`.
 * - Model registry: `server/src/llm_router/model_registry.rs` `ModelMapping`.
 * - Secrets: `server/src/secrets/routes.rs` `SecretEntry` (names only, never values).
 * - Classifier: `server/src/llm_router/classifier.rs` `ClassifierStatusResponse`, `PreviewRequest`,
 *   `PreviewResponse` (in OpenAPI); `diagnostics` mirrors `llm-router` `BackendDiagnostics` (untyped `Object` there).
 */
import type { components } from '@/lib/api/schema.gen'

type S = components['schemas']

/** One `llm_configs` row as `CONFIG_JSON` builds it. */
export interface LlmConfig {
  id: string
  name: string
  provider: string
  model: string | null
  fallback_models: string[]
  temperature: number | null
  max_tokens: number | null
  api_key_secret_name: string | null
  pinned: boolean
  pinned_model: string | null
  tier1_model: string | null
  tier2_model: string | null
  tier3_model: string | null
  is_default: boolean
  created_at: string
  updated_at: string
}

export type RoutingSource = 'attached' | 'owner-default' | 'none'

/** A config as the agent routing response nests it (without its timestamps). */
export type RoutingConfig = Omit<LlmConfig, 'created_at' | 'updated_at'>

/** GET /api/agents/{id}/llm-config `data`. `llm_config` is the resolved config (attached, else the owner's default). */
export interface AgentRouting {
  agent_id: string
  llm_config_id: string | null
  /** The server's CONFIG_JSON subset (agents/llm_config.rs, ea233d20): no created_at/updated_at. */
  llm_config: RoutingConfig | null
  source: RoutingSource
  inbound_format: string
  pinned_model: string | null
}

export type CreateConfigBody = S['CreateLlmConfigRequest']
export type UpdateConfigBody = S['UpdateLlmConfigRequest']
/** Double-option fields: absent leaves the value, null clears (detach / remove override), a value sets it. */
export type AttachBody = S['AttachLlmConfigRequest']
export type ProviderCatalog = S['ProviderCatalog']

export interface ModelMapping {
  provider: string
  tier: number
  model: string
}

export interface SecretEntry {
  id: string
  name: string
  created_at: string
  updated_at: string
}

/** The wire dialect a custom provider speaks (`dialect.rs` `KIND_*`). Immutable once registered. */
export type ProviderKind = 'openai' | 'azure-openai' | 'bedrock-converse'

export interface CustomProvider {
  id: string
  label: string
  display_name: string
  base_url: string
  kind?: ProviderKind
  /** Azure `api-version`; null for every other kind. */
  api_version?: string | null
  default_model: string | null
  catalog_sync_enabled: boolean
  api_key_set: boolean
  last_sync_at: string | null
  last_sync_status: string | null
  last_sync_error: string | null
  created_at: string
}

export interface CreateCustomProviderBody {
  display_name: string
  base_url: string
  kind: ProviderKind
  /** Required for `azure-openai`, ignored otherwise. */
  api_version?: string
  api_key: string
  default_model?: string | null
  catalog_sync_enabled: boolean
}

/** `kind` can't change on a PATCH (the server ignores it). */
export type UpdateCustomProviderBody = Partial<
  Omit<CreateCustomProviderBody, 'catalog_sync_enabled' | 'kind'>
> & { catalog_sync_enabled?: boolean }

export interface TestCustomProviderBody {
  base_url: string
  api_key: string
  kind: ProviderKind
  api_version?: string
  model?: string | null
}

export interface TestCustomProviderResult {
  chat_ok: boolean
  chat_error?: string
  models: string[]
}

export type AgentUsage = S['AgentUsage']

// ─── R2 budgets: the proposed R-L10 contract (not in the server yet; the `router` mock group answers it) ──────────

type BudgetScope = 'owner' | 'agent'
export type BudgetAction = 'alert' | 'stop'
export type BudgetState = 'ok' | 'warning' | 'exceeded'

export interface Budget {
  id: string
  scope: BudgetScope
  /** Set for an agent budget, null for the owner budget. */
  agent_id: string | null
  /** The user it belongs to: the owner budget's user, or the agent's owner, whoever set it. */
  owner_id: string
  /** Who created or last changed it (a superuser may set a budget on anyone's agent). */
  set_by: string
  period: 'month'
  limit_usd: number
  /** Whole percents of the limit, ascending and unique (default 50, 80, 100); a `stop` budget always has 100. */
  thresholds: number[]
  action: BudgetAction
  created_at: string
  updated_at: string
}

export interface CreateBudgetBody {
  scope: BudgetScope
  agent_id?: string | null
  limit_usd: number
  thresholds?: number[]
  action?: BudgetAction
}

/** A full replace (R-L10 has no COALESCE), guarded against a change made elsewhere since it was read. */
export interface UpdateBudgetBody {
  limit_usd: number
  thresholds: number[]
  action: BudgetAction
  /** The `updated_at` the client read; a mismatch is 409 "budget changed elsewhere". */
  expected_updated_at: string
}

export interface BudgetStatus {
  budget_id: string
  used_usd: number
  /** Calls this period with no price (NULL cost_usd): they count as $0. */
  unpriced_calls: number
  resets_at: string
  state: BudgetState
  /** The highest threshold crossed this period, or null. */
  crossed: number | null
  /** One entry per UTC day of the period so far, zeros included. */
  daily: { date: string; cost_usd: number }[]
  /** A `stop` budget the router is refusing calls for right now (its effective state, after its cache window). */
  stopped: boolean
}

export interface BudgetStatusResponse {
  data: BudgetStatus[]
  covers: 'routed_calls'
}

export interface BudgetAlert {
  id: string
  budget_id: string
  threshold: number
  amount_usd: number
  at: string
  stopped: boolean
}

// ─── Request classifier ([classifier] companion): status and side-effect-free preview ──────────────

export type ClassifierStatus = S['ClassifierStatusResponse']
export type ClassifierPreviewBackend = S['PreviewBackend']
export type ClassifierPreviewBody = S['PreviewRequest']

/** `llm-router` `routing::classifier::BackendDiagnostics`: present only when a hosted backend answered. */
export interface ClassifierDiagnostics {
  model_version: string | null
  type_probabilities: [string, number][]
  complexity_probabilities: number[]
  complexity_expected: number | null
  vendor_type_confidence: number | null
  vendor_complexity_confidence: number | null
  input_tokens: number | null
  output_tokens: number | null
  attempts: number
  /** The backend cut the input to its own window (Laya's 512-token budget). */
  input_truncated?: boolean
}

export type ClassifierPreviewResult = Omit<S['PreviewResult'], 'diagnostics'> & {
  diagnostics: ClassifierDiagnostics | null
}

export type ClassifierPreview = Omit<S['PreviewResponse'], 'result' | 'baseline'> & {
  result: ClassifierPreviewResult
  baseline: ClassifierPreviewResult
}
