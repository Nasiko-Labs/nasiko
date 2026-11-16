import type { BrowserOptions, Breadcrumb, ErrorEvent } from '@sentry/react'
import { ApiError, isAbortError, isTimeoutError } from '@nasiko/data-access/client'

/**
 * What a report may say, applied in the browser before anything is sent. The server's tunnel can't read a compressed
 * replay, so this is the place that decides. The query-key rule is the server's own (`mcp::redact_credential_uri`),
 * so a URL is redacted the same way in a span, a log line and a crash report.
 */

/**
 * Not exported by @sentry/react 11; the span its `beforeSendSpan` receives. SDK 11 streams spans one by one
 * (`traceLifecycle: 'stream'`, the default) and ignores `beforeSendTransaction`, so spans are scrubbed here.
 */
export type StreamedSpan = Parameters<NonNullable<BrowserOptions['beforeSendSpan']>>[0]

const SENSITIVE_QUERY_KEY_PARTS = [
  'token',
  'secret',
  'password',
  'passwd',
  'key',
  'code',
  'state',
  'sig',
  'auth',
  'session',
  'credential',
  'jwt',
]
const REDACTED = '[REDACTED]'
const CREDENTIAL_PATH = '/api/mcp/s/'

const isSensitiveKey = (name: string) => {
  const lower = name.toLowerCase()
  return SENSITIVE_QUERY_KEY_PARTS.some((part) => lower.includes(part))
}

/** A URL with credentials taken out of its path and query; anything that isn't a URL comes back unchanged. */
export function redactUrl(raw: string): string {
  if (!/^(https?:\/\/|\/)/i.test(raw)) return raw
  let url: URL
  try {
    url = new URL(raw, 'http://relative.invalid')
  } catch {
    return raw
  }
  const relative = url.origin === 'http://relative.invalid'
  if (url.pathname.startsWith(CREDENTIAL_PATH)) {
    url.pathname = `${CREDENTIAL_PATH}{token}`
    url.search = ''
  }
  const pairs = [...url.searchParams.keys()]
  if (pairs.some(isSensitiveKey)) {
    const next = new URLSearchParams()
    url.searchParams.forEach((value, name) =>
      next.append(name, isSensitiveKey(name) ? REDACTED : value),
    )
    url.search = next.toString()
  }
  url.hash = ''
  const text = url.toString().replaceAll(encodeURIComponent(REDACTED), REDACTED)
  return relative ? text.slice('http://relative.invalid'.length) : text
}

/**
 * Failures the page already explains and that are not bugs: a 4xx the server meant (not found, no access, a
 * validation message), a cancelled or timed-out request, and the network being down. Reporting them would bury the
 * real crashes.
 */
export function isExpectedFailure(error: unknown): boolean {
  if (isAbortError(error) || isTimeoutError(error)) return true
  if (!(error instanceof ApiError)) return false
  // apiFetch's "network error" is a 502 with no body: the browser never reached the server.
  if (error.status === 502 && error.body === null) return true
  return error.status >= 400 && error.status < 500 && error.status !== 429
}

function scrubRequest(event: ErrorEvent): ErrorEvent {
  const request = event.request
  if (!request) return event
  const headers = request.headers ?? {}
  const referer = headers.Referer ?? headers.referer
  return {
    ...event,
    request: {
      ...request,
      url: request.url ? redactUrl(request.url) : request.url,
      query_string: undefined,
      cookies: undefined,
      data: undefined,
      headers: {
        ...(headers['User-Agent'] ? { 'User-Agent': headers['User-Agent'] } : {}),
        ...(referer ? { Referer: redactUrl(referer) } : {}),
      },
    },
  }
}

/** `beforeSend`: drop expected failures, scrub the rest. */
export function scrubEvent(
  event: ErrorEvent,
  hint: { originalException?: unknown },
): ErrorEvent | null {
  if (isExpectedFailure(hint.originalException)) return null
  return scrubRequest(event)
}

/** A bare query string (`a=1&token=x`, with or without `?`), redacted with the same key rule as a URL. */
function redactQuery(raw: string): string {
  const query = raw.startsWith('?') ? raw.slice(1) : raw
  const redacted = redactUrl(`/?${query}`).slice(2)
  return raw.startsWith('?') && redacted ? `?${redacted}` : redacted
}

const QUERY_ATTRIBUTES = new Set(['url.query', 'http.query'])
const KEPT_REQUEST_HEADERS = new Set([
  'http.request.header.user_agent',
  'http.request.header.user-agent',
])

/**
 * `beforeSendSpan`. A span can't be dropped here, only cleaned: every URL in its name and string attributes is
 * redacted, bare query attributes too, and request/response headers go except the user agent. An allow-list of
 * attribute keys would break whenever the SDK adds one; redacting by value catches a URL under any key.
 */
export function scrubSpan(span: StreamedSpan): StreamedSpan {
  const attributes: StreamedSpan['attributes'] = {}
  for (const [key, value] of Object.entries(span.attributes)) {
    if (key.startsWith('http.response.header.')) continue
    if (key.startsWith('http.request.header.') && !KEPT_REQUEST_HEADERS.has(key)) continue
    if (typeof value !== 'string') attributes[key] = value
    else attributes[key] = QUERY_ATTRIBUTES.has(key) ? redactQuery(value) : redactUrl(value)
  }
  // An http.client span is named "GET https://host/path?query".
  const name = span.name.split(' ').map(redactUrl).join(' ')
  return { ...span, name, attributes }
}

const URL_FIELDS = ['url', 'from', 'to'] as const

/** `beforeBreadcrumb`: navigation and request URLs redacted; console lines kept only as their level and first line. */
export function scrubBreadcrumb(crumb: Breadcrumb): Breadcrumb | null {
  const data = crumb.data ? { ...crumb.data } : undefined
  if (data) {
    for (const field of URL_FIELDS) {
      const value: unknown = data[field]
      if (typeof value === 'string') data[field] = redactUrl(value)
    }
  }
  if (crumb.category === 'console') {
    const first = (crumb.message ?? '').split('\n')[0] ?? ''
    return { ...crumb, message: first.slice(0, 200), data: undefined }
  }
  return { ...crumb, data }
}

/** What `beforeSendLog` receives: one console line on its way to Sentry Logs. */
export type LogRecord = Parameters<NonNullable<BrowserOptions['beforeSendLog']>>[0]

const EMAIL = /[^\s@<>()"',;]+@[^\s@<>()"',;]+\.[a-z]{2,}/gi
const URL_IN_TEXT = /(?:https?:\/\/|\/)[^\s"'<>()]*\?[^\s"'<>()]*|https?:\/\/[^\s"'<>()]+/gi

/** Free text with every URL redacted (the same rule as everywhere else) and email addresses replaced. */
export function redactText(text: string): string {
  return text.replace(URL_IN_TEXT, (url) => redactUrl(url)).replace(EMAIL, '[email]')
}

/**
 * `beforeSendLog`. Console lines are free text nobody reviewed for what Sentry may see: URLs and email addresses are
 * redacted in the message and in every string attribute, and an attribute whose name looks secret keeps only a
 * placeholder. The SDK's own `sentry.*` attributes and the user id it adds are left alone.
 */
export function scrubLog(log: LogRecord): LogRecord {
  const attributes: Record<string, unknown> = {}
  for (const [key, value] of Object.entries(log.attributes ?? {})) {
    if (typeof value !== 'string' || key.startsWith('sentry.') || key === 'user.id')
      attributes[key] = value
    else attributes[key] = isSensitiveKey(key) ? REDACTED : redactText(value)
  }
  const message =
    typeof log.message === 'string' ? redactText(log.message) : redactText(String(log.message))
  return { ...log, message, attributes }
}
