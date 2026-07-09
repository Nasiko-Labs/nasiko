// Single funnel for control-plane API calls.
//
// Every request to /api/* must go through apiFetch (or its JSON wrapper
// fetchApi) so that "session missing or expired" is handled in exactly one
// place. Do not call window.fetch('/api/...') directly from components.
//
// The auth cookie is HttpOnly — the UI cannot inspect it, so auth state is
// only ever discovered from a server response. A server-side redirect would
// be wrong for API calls (fetch() follows it transparently and hands the
// caller login-page HTML), so the API returns 401 and we navigate the page.

/// Low-level: performs the request and handles 401 by redirecting to the
/// login page. Returns the raw Response for callers that stream, read text,
/// or branch on status themselves.
export async function apiFetch(path, opts = {}) {
  const res = await fetch(`/api${path}`, opts);
  if (res.status === 401 && !window.location.pathname.startsWith('/login')) {
    window.location.href = '/login.html';
    return new Promise(() => {}); // page is navigating away; never settle
  }
  return res;
}

/// Convenience: JSON in, JSON out, throws on any non-2xx.
export async function fetchApi(path, opts = {}) {
  const res = await apiFetch(path, opts);
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new Error(text || `HTTP ${res.status}`);
  }
  return res.json();
}
