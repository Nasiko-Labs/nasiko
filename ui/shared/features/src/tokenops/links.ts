/**
 * Link to the existing nasiko UI's session/trace view (plan F4, A18). Built only from a
 * validated env base plus URLSearchParams-encoded ids — never from response strings.
 * Relative to the base so a UI mounted under a sub-path keeps its prefix.
 * Returns null when there is no valid base, and the UI hides the link.
 */
export function traceLink(
  base: string | null,
  sessionId: string | null,
  traceId: string,
): string | null {
  if (!base) return null
  const url = new URL('observability-session', base.endsWith('/') ? base : `${base}/`)
  const qs = new URLSearchParams()
  if (sessionId) qs.set('session_id', sessionId)
  qs.set('trace_id', traceId)
  url.search = qs.toString()
  return url.toString()
}
