/**
 * Settings' pure rules (plans/feat-settings.md §3), each mirroring nasiko-cloud-rs `43833316` so a form says what the
 * server would say before the request goes out. Tested in `logic.test.ts`.
 */
import { SETTINGS_FIELDS, type Settings, type SettingsField } from './types'

// ─── workspace settings (oss/server/src/settings.rs) ────────────────────────

/** The form holds every field as text; numbers are sent as numbers, blanks as null. */
export type SettingsValues = Record<SettingsField, string>

const NUMERIC: ReadonlySet<SettingsField> = new Set([
  'max_flow_depth',
  'max_flow_fan_out',
  'max_flow_tokens',
  'flow_timeout_secs',
])

export const valuesOf = (s: Settings): SettingsValues =>
  Object.fromEntries(
    SETTINGS_FIELDS.map((k) => [k, s[k] == null ? '' : String(s[k])]),
  ) as SettingsValues

/**
 * The PUT body. `update_settings` writes every column, so the body carries all of them: the fields the user edited
 * from the form, every other one from a FRESH read, so another admin's save (or a column the form never showed) is
 * sent back unchanged instead of overwritten with the page's stale copy or nulled (ST-2).
 */
export function settingsBody(
  fresh: Settings,
  values: SettingsValues,
  edited: ReadonlySet<SettingsField>,
): Record<SettingsField, string | number | null> {
  const body = {} as Record<SettingsField, string | number | null>
  for (const k of SETTINGS_FIELDS) {
    if (!edited.has(k)) {
      body[k] = fresh[k] ?? null
      continue
    }
    const v = values[k].trim()
    body[k] = !v ? null : NUMERIC.has(k) ? Number(v) : v
  }
  return body
}

/** A column's upper bound: INT, or BIGINT held as a safe JS integer. */
export const INT_MAX = 2_147_483_647

/** A whole number from 1 to `max`. */
export const positiveIntProblem = (v: string, max: number): 'required' | 'invalid' | null =>
  !v.trim()
    ? 'required'
    : /^\d+$/.test(v.trim()) && Number(v) >= 1 && Number(v) <= max
      ? null
      : 'invalid'

/**
 * The OCI registry field: blank, or a host (optional port) with or without a scheme and path. The import allow list
 * takes only its host (catalog/import.rs `registry_url_host`); the server stores anything, so a typo would silently
 * allow nothing (ST-4).
 */
export function registryProblem(url: string): 'invalid' | null {
  const s = url.trim()
  if (!s) return null
  if (/\s/.test(s)) return 'invalid'
  const host = (s.replace(/^https?:\/\//, '').split('/')[0] ?? '').trim()
  return /^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*(:\d{1,5})?$/i.test(host)
    ? null
    : 'invalid'
}

// ─── secrets (oss/server/src/secrets/routes.rs validate_secret_name) ────────

/** Names become container env-var keys at deploy, so the server refuses ones that change how a runtime behaves. */
const RESERVED_SECRET_NAMES: ReadonlySet<string> = new Set([
  'PATH',
  'LD_PRELOAD',
  'LD_LIBRARY_PATH',
  'LD_AUDIT',
  'DYLD_INSERT_LIBRARIES',
  'DYLD_LIBRARY_PATH',
  'IFS',
  'HOME',
  'SHELL',
  'BASH_ENV',
  'ENV',
  'PYTHONPATH',
  'NODE_OPTIONS',
  'PERL5LIB',
  'GIT_SSH_COMMAND',
])

export type SecretNameProblem = 'required' | 'length' | 'pattern' | 'reserved'

/** The server's rule (a 422 otherwise). Null when the name is fine. */
export function secretNameProblem(name: string): SecretNameProblem | null {
  if (!name) return 'required'
  if (name.length > 128) return 'length'
  if (!/^[A-Z_][A-Z0-9_]*$/.test(name)) return 'pattern'
  if (RESERVED_SECRET_NAMES.has(name)) return 'reserved'
  return null
}

/** The caller's router configs whose provider key is this secret (`llm_configs.api_key_secret_name`). */
export const configsUsing = (
  configs: readonly { name: string; api_key_secret_name: string | null }[] | undefined,
  secret: string,
): string[] => (configs ?? []).filter((c) => c.api_key_secret_name === secret).map((c) => c.name)

/** "3d ago", as the legacy list says it (ui/common/utils/date-utils.js `timeAgo`). */
export function ago(iso: string, now: number): string {
  const s = Math.max(0, Math.floor((now - Date.parse(iso)) / 1000))
  if (s < 60) return `${s}s ago`
  if (s < 3600) return `${Math.floor(s / 60)}m ago`
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`
  return `${Math.floor(s / 86400)}d ago`
}
