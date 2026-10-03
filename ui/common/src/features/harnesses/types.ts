/**
 * Wire types for the PROPOSED `GET /api/observability/coding-agents/usage` endpoint
 * (plans/feat-harness-org-view.md §4). nasiko-server does not have it yet: the lab mocks
 * it, and docs/designs/openruntime-harness-recommendations.md proposes it upstream.
 * Errors and pagination follow nasiko-cloud-rs oss/docs/API_CONVENTIONS.md (cb3aaf0c).
 */

/** A `coding_agent_integration_id`; unknown ids are kept (Decision 2: data-driven set). */
export type HarnessId = string

export interface HarnessTotals {
  /** Distinct in-scope active, non-deleted users, including those with nothing registered. */
  scope_devs: number
  active_devs: number
  /** ≥1 non-deleted agent row for (owner, harness): created by install or auto-detection on connect/login. */
  registered_devs: number
  /** Registered (developer, harness) pairs with no activity in the window: one meaning
   *  everywhere (API_CONVENTIONS §4). A developer idle on two harnesses is two idle seats. */
  idle_seats: number
  sessions: number
  turns: number
  tokens: number
  /** Tokens × model_pricing list price — an estimate, not an invoice. */
  cost_usd: number
  unpriced_calls: number
  /** Change in TURNS vs the previous window (activity, same as the live fallback); null when compare is off or previous is 0. */
  delta_pct: number | null
}

type ScopeKind = 'org' | 'mine' | 'unassigned' | 'unit' | 'direct' | 'user'
export type RowKind = 'unit' | 'user' | 'unassigned' | 'direct'

export interface UsageScope {
  kind: ScopeKind
  unit_id?: string
  user_id?: string
  label: string
  visibility: 'named' | 'aggregated'
  /** Individual level: the developer's unit placement (for the breadcrumb on a direct load). */
  units?: { id: string; name: string }[]
}

export interface UsageRow {
  key: string
  label: string
  kind: RowKind
  harness_breakdown: Record<HarnessId, HarnessTotals>
  totals: HarnessTotals
  last_active?: string
}

export interface UsageSeriesPoint {
  date: string
  harness: HarnessId
  active_devs: number
  cost_usd: number
  tokens: number
}

export interface RecentSession {
  session_id: string
  harness: HarnessId
  started_at: string
  turns: number
  cost_usd: number
}

export interface UsageResponse {
  scope: UsageScope
  window: { start_time: string; end_time: string; range?: '24h' | '7d' | '30d' }
  totals: HarnessTotals
  by_harness: (HarnessTotals & { harness: HarnessId; top_models: string[] })[]
  /** Children of the scope; empty at the Individual level. */
  rows: UsageRow[]
  /** Distinct developers who appear in two or more `rows` (the parent totals count them once). */
  overlap_devs: number
  /** Always present, so the trend never makes a second request. */
  series: UsageSeriesPoint[]
  recent_sessions?: RecentSession[]
  /** API_CONVENTIONS §1 paging, always present. Only developer lists (group_by=user) page;
   *  cursors are opaque keyset cursors over (display_name, user_id). */
  has_more: boolean
  next_cursor: string | null
  prev_cursor: string | null
  /** Full filtered count of developers, group_by=user only. */
  total_count?: number
}

type UsageErrorCode =
  | 'unit_not_visible'
  | 'user_not_visible'
  | 'not_found'
  | 'invalid_scope'
  | 'invalid_range'
  | 'invalid_window'
  | 'invalid_group_by'
  | 'missing_unit_id'
  | 'invalid_cursor'
  | 'internal'

/** API_CONVENTIONS error shape. A 404 WITHOUT this body means the endpoint is absent. */
export interface UsageError {
  error: string
  code: UsageErrorCode
}

/** `UnitRow` from EE `GET /api/org/units` (ee/server/src/org_units.rs UNIT_COLUMNS, cb3aaf0c). No `path`. */
export interface UnitRow {
  id: string
  parent_id: string | null
  name: string
  depth: number
  lead_id: string | null
  lead_username: string | null
  source: string
  provider: string | null
  external_id: string | null
  idp_synced_at: string | null
  member_count: number
  created_at: string
}

/** EE `GET /api/users/me` (ee/server/src/users.rs get_me). OSS returns the same without unit arrays. */
export interface UserMe {
  id: string
  username: string
  email: string | null
  display_name: string | null
  is_superuser: boolean
  is_active: boolean
  role: string
  /** EE only (ee/server/src/users.rs UserRow at ea233d20): "credentials" for local accounts. */
  auth_provider?: string | null
  unit_ids?: string[]
  unit_names?: string[]
  created_at: string
  last_login: string | null
}
