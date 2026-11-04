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
