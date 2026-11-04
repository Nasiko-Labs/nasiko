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
 * `GET/PUT /api/settings/composio` (`oss/server/src/composio_settings.rs`): not in the OpenAPI spec. The key itself is
 * never returned, only whether one is in effect and where it comes from.
 */
export const composioSchema = z.looseObject({
  api_key_configured: z.boolean(),
  key_source: z.enum(['ui', 'env', 'none']),
  toolkits: z.array(
    z.looseObject({
      toolkit: z.string(),
      display_name: z.string(),
      description: text,
      logo_url: text,
      enabled: z.boolean(),
    }),
  ),
  /** Ticked toolkits Composio refused to register. */
  failed: z.array(z.looseObject({ toolkit: z.string(), error: z.string() })),
})
export type ComposioSettings = z.infer<typeof composioSchema>
export type ComposioToolkit = ComposioSettings['toolkits'][number]

/** `ComposioSettingsUpdate`: every field optional; leaving one out keeps what is stored. */
export interface ComposioUpdate {
  api_key?: string
  clear_api_key?: boolean
  toolkits?: string[]
}
