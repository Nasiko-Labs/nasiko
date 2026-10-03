/**
 * Thin fetch wrapper for the nasiko-server API.
 *
 * Requests are same-origin: Vite proxies /api to the OSS server (see
 * vite.config.ts), so the HttpOnly `access_token` cookie set by
 * POST /api/auth/login rides along automatically. No token handling here.
 *
 * In mock mode MSW intercepts the same requests (src/mocks).
 *
 * Responses are cast to `T` unless the call passes a zod `schema` (plan §8 Phase 8 item 5): then a body
 * that fails it throws an ApiError, so the page shows its error state instead of rendering partial data.
 */
import type { ZodType } from 'zod'

export class ApiError extends Error {
  readonly status: number
  readonly body: unknown
  readonly path: string
  /** Seconds from the response's `Retry-After` header (429/503), when it sent one. */
  readonly retryAfterSeconds: number | null

  constructor(
    status: number,
    body: unknown,
    path: string,
    message: string,
    retryAfterSeconds: number | null = null,
  ) {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.body = body
    this.path = path
    this.retryAfterSeconds = retryAfterSeconds
  }

  /** A 403 is never session loss (only a 401 is): render an inline "No access" state. */
  get isForbidden(): boolean {
    return this.status === 403
  }

  /** Worth retrying: the server (or the proxy in front of it) failed, timed out or rate-limited. Never another 4xx. */
  get isRetryable(): boolean {
    return this.status >= 500 || this.status === 408 || this.status === 429
  }

  /** The server's human-readable reason, when it sent one (finops 4xx/5xx are often plain text). */
  get serverMessage(): string | null {
    if (typeof this.body === 'string' && this.body.trim()) return this.body.trim()
    if (this.body && typeof this.body === 'object') {
      const b = this.body as { message?: unknown; error?: unknown }
      if (typeof b.message === 'string') return b.message
      if (typeof b.error === 'string') return b.error
      // JSON-RPC errors (POST /api/orchestrator/a2a): `{error: {code, message}}`.
      if (
        b.error &&
        typeof b.error === 'object' &&
        typeof (b.error as { message?: unknown }).message === 'string'
      )
        return (b.error as { message: string }).message
    }
    return null
  }

  /**
   * The Vite proxy answers 502/504 when nasiko-server is not running, and apiFetch maps
   * a network-level fetch failure to 502 too. The ServerDown page keys on this.
   */
  get isServerUnreachable(): boolean {
    return this.status === 502 || this.status === 504
  }
}

/** By name, not `instanceof DOMException`: an error from another realm (jsdom, workers) fails instanceof. */
export const isAbortError = (err: unknown) =>
  (err as { name?: unknown } | null)?.name === 'AbortError'
/** A deadline firing: apiFetch's own, or a caller's `AbortSignal.timeout` (same by-name rule as isAbortError). */
export const isTimeoutError = (err: unknown) =>
  (err as { name?: unknown } | null)?.name === 'TimeoutError'

/** Retry-worthy: a retryable ApiError (never a 401, which is session loss) or a deadline. Aborts never are. */
export const isRetryable = (err: unknown) =>
  (err instanceof ApiError && err.status !== 401 && err.isRetryable) || isTimeoutError(err)

/** Every call's deadline unless it passes its own `timeout` (streams don't use apiFetch). */
const API_TIMEOUT_MS = 30_000

export interface ApiInit extends RequestInit {
  /** ms; 0 turns the deadline off. It combines with the caller's `signal`, and fires as a `TimeoutError`. */
  timeout?: number
}

/** `T` with every field optional, all the way down: a schema checks only the fields the UI reads. */
export type WireSubset<T> = T extends readonly (infer U)[]
  ? WireSubset<U>[]
  : T extends object
    ? { [K in keyof T]?: WireSubset<T[K]> }
    : T

/**
 * `schema` checks the body (apiData: its `data`). Its fields must agree with `T` (a compile error otherwise), and it
 * should be `z.looseObject` all the way down, so a field the server adds never fails. It checks and never
 * transforms: the body is returned as the server sent it.
 */
export interface Checked<T> {
  schema?: ZodType<WireSubset<T>>
}

/** A body that fails its schema is an ApiError (status 200, never retried); dev builds log where it failed. */
function check(schema: ZodType, value: unknown, path: string, method = 'GET'): void {
  const r = schema.safeParse(value)
  if (r.success) return
  const where = r.error.issues.slice(0, 5).map((i) => `${i.path.join('.') || '$'}: ${i.message}`)
  if (import.meta.env.DEV) console.error(`${method} ${path}: the response failed its schema`, where)
  throw new ApiError(200, value, path, `${method} ${path} → unexpected response (${where[0]})`)
}

export async function apiFetch<T>(path: string, init: ApiInit & Checked<T> = {}): Promise<T> {
  const { timeout = API_TIMEOUT_MS, signal, schema, ...rest } = init
  const signals = [signal, timeout ? AbortSignal.timeout(timeout) : null].filter(
    (s): s is AbortSignal => !!s,
  )
  let res: Response
  try {
    // Absolute against the page origin: identical in the browser, and required by
    // Node's fetch under Vitest (it rejects relative URLs).
    res = await fetch(new URL(path, globalThis.location?.origin ?? 'http://localhost'), {
      ...rest,
      signal: signals.length > 1 ? AbortSignal.any(signals) : signals[0],
      credentials: 'same-origin',
      headers: { Accept: 'application/json', ...rest.headers },
    })
  } catch (err) {
    // Aborts and timeouts are rethrown, so the caller can say "cancelled" or "slow", not "down".
    if (isAbortError(err) || isTimeoutError(err)) throw err
    throw new ApiError(502, null, path, `${rest.method ?? 'GET'} ${path} → network error`)
  }
  const text = await res.text()
  const body: unknown = text ? safeJson(text) : null
  if (!res.ok)
    throw new ApiError(
      res.status,
      body,
      path,
      `${rest.method ?? 'GET'} ${path} → ${res.status}`,
      retryAfter(res.headers.get('retry-after')),
    )
  if (schema) check(schema, body, path, rest.method)
  return body as T
}

/** `Retry-After` in seconds (delta form; an HTTP date counts from now). */
function retryAfter(raw: string | null): number | null {
  if (!raw) return null
  const secs = /^\d+$/.test(raw.trim()) ? Number(raw) : (Date.parse(raw) - Date.now()) / 1000
  return Number.isFinite(secs) && secs > 0 ? Math.ceil(secs) : null
}

/** For endpoints that wrap their payload in `{data, status_code, message}`. */
export async function apiData<T>(path: string, init: ApiInit & Checked<T> = {}): Promise<T> {
  const { schema, ...rest } = init
  const body = await apiFetch<unknown>(path, rest)
  // An empty 204, an HTML fallback page or a changed wire format must fail loudly,
  // not turn into `undefined` cast to T.
  if (
    !body ||
    typeof body !== 'object' ||
    !('data' in body) ||
    (body as { data: unknown }).data == null
  ) {
    throw new ApiError(
      200,
      body,
      path,
      `${init.method ?? 'GET'} ${path} → unexpected response (no data envelope)`,
    )
  }
  const data = (body as { data: unknown }).data
  if (schema) check(schema, data, path, init.method)
  return data as T
}

/** Build `path?k=v` from a params object, dropping undefined/null/empty values. */
export function withQuery(
  path: string,
  params: Record<string, string | number | boolean | null | undefined>,
): string {
  const qs = new URLSearchParams()
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === null || v === '') continue
    qs.set(k, String(v))
  }
  const s = qs.toString()
  return s ? `${path}?${s}` : path
}

export function safeJson(text: string): unknown {
  try {
    return JSON.parse(text)
  } catch {
    return text
  }
}
