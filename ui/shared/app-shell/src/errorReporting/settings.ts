import { apiFetch } from '@nasiko/data-access/client'

/**
 * What the server says about error reporting (`GET /api/telemetry/config`, its `error_reporting` module). It is runtime
 * configuration, not a build setting: one bundle ships to every deployment, and a self-hosted install reports nothing
 * unless its operator sets `SENTRY_BROWSER_DSN`.
 */
export interface ErrorReportingSettings {
  dsn: string
  environment: string
  tunnel: string
  tracesSampleRate: number
  replaysSessionSampleRate: number
  replaysOnErrorSampleRate: number
  /** Warnings and errors from the console go to Sentry Logs too (the server's `SENTRY_LOGS_LEVEL` is not `off`). */
  logs: boolean
  /** The workspace this control plane serves (its `TENANT_ID`), tagged on every report; null on a self-hosted install. */
  workspace: string | null
}

/** The settings, or null when reporting is off. Throws when the server can't be asked (the caller then stays off). */
export async function loadErrorReportingSettings(): Promise<ErrorReportingSettings | null> {
  return parseSettings(await apiFetch<unknown>('/api/telemetry/config', { timeout: 10_000 }))
}

/**
 * Checked by hand rather than with a schema: this file is in the shell, and the shape is small. Anything unexpected
 * means "off", never a half-configured SDK.
 */
export function parseSettings(body: unknown): ErrorReportingSettings | null {
  const data = (body as { data?: Record<string, unknown> } | null)?.data
  if (!data || data.enabled !== true) return null
  const { dsn, environment, tunnel } = data
  if (typeof dsn !== 'string' || typeof environment !== 'string' || typeof tunnel !== 'string')
    return null
  if (!tunnel.startsWith('/api/')) return null
  return {
    dsn,
    environment,
    tunnel,
    tracesSampleRate: rate(data.traces_sample_rate),
    replaysSessionSampleRate: rate(data.replays_session_sample_rate),
    replaysOnErrorSampleRate: rate(data.replays_on_error_sample_rate),
    logs: data.logs === true,
    workspace: typeof data.workspace === 'string' && data.workspace ? data.workspace : null,
  }
}

const rate = (value: unknown) => (typeof value === 'number' && value >= 0 && value <= 1 ? value : 0)
