/**
 * Wire types for Settings (plans/feat-settings.md §2). `/api/settings` is not in the
 * OpenAPI spec (ST-6), so it is a zod schema here. Secrets reuse the router's `SecretEntry` (in the spec).
 * Server: nasiko-cloud-rs `43833316`, `oss/server/src/settings.rs` `Settings` / `SettingsUpdate`.
 */
import { z } from 'zod'

const text = z.string().nullable().optional()
const int = z.number().int().nullable().optional()

/** `Settings`: a bare JSON object (no envelope). Every field is nullable; the row is a singleton. */
export const settingsSchema = z.looseObject({
  router_model: text,
  default_provider: text,
  max_flow_depth: int,
  max_flow_fan_out: int,
  max_flow_tokens: int,
  flow_timeout_secs: int,
  registry_url: text,
  catalog_tabs: text,
})
export type Settings = z.infer<typeof settingsSchema>

/**
 * `SettingsUpdate`'s fields. The PUT binds every one of them (`INSERT … ON CONFLICT DO UPDATE SET` all columns), so a
 * field left out is written as NULL (ST-2): the body always carries all of them.
 */
export const SETTINGS_FIELDS = [
  'router_model',
  'default_provider',
  'max_flow_depth',
  'max_flow_fan_out',
  'max_flow_tokens',
  'flow_timeout_secs',
  'registry_url',
  'catalog_tabs',
] as const satisfies readonly (keyof Settings)[]

export type SettingsField = (typeof SETTINGS_FIELDS)[number]

/**
 * Chat context: the caller's own history-selection preferences, `GET/PATCH /api/me/context-strategy` and
 * `/api/me/pacms-budget` (nasiko-cloud-rs `1a305a63`, `oss/server/src/context_selection.rs`
 * `ContextStrategyResponse` / `PacmsBudgetResponse`; enums in `oss/orchestrator/src/context_selection.rs`). Not in
 * the OpenAPI spec. Any signed-in user; a value outside the enum is Axum's 422.
 */
export const CONTEXT_STRATEGIES = ['pacms', 'topk', 'lastk'] as const
export const BUDGET_LEVELS = ['low', 'medium', 'high'] as const
export type ContextStrategy = (typeof CONTEXT_STRATEGIES)[number]
export type BudgetLevel = (typeof BUDGET_LEVELS)[number]
export const contextStrategySchema = z.looseObject({ strategy: z.enum(CONTEXT_STRATEGIES) })
export const pacmsBudgetSchema = z.looseObject({ level: z.enum(BUDGET_LEVELS) })
