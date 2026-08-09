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

/// Multi-tenant seam: the BFF dashboard injects
/// `window.nasikoConfig = { apiBase: "https://<sub>.nasiko.dev", ... }` at
/// serve time (docs/MULTITENANT.md §9). Empty/absent = same-origin (single
/// tenant, today's behavior). Resolved per call — never cached — so an
/// in-SPA workspace switch re-points every subsequent request.
function apiBase() {
  return window.nasikoConfig?.apiBase || "";
}

/// Low-level: performs the request and handles 401 by redirecting to the
/// login page. Returns the raw Response for callers that stream, read text,
/// or branch on status themselves.
export async function apiFetch(path, opts = {}) {
  const base = apiBase();
  // Cross-origin CP calls ride the CP's host-only session cookie.
  const withCreds = base ? { credentials: "include", ...opts } : opts;
  const res = await fetch(`${base}/api${path}`, withCreds);
  if (res.status === 401 && !window.location.pathname.startsWith('/login')) {
    // Remember where we were so the dashboard can return here after re-auth (the
    // OAuth round-trip lands back on `/`; the BFF-injected restore snippet reads
    // this and replaces to the deep link). sessionStorage survives the same-tab
    // round-trip to the IdP and back. Timestamped so a stale value can't bounce a
    // later deliberate visit to `/`.
    try {
      sessionStorage.setItem(
        'nasiko:returnTo',
        JSON.stringify({ p: location.pathname + location.search, t: Date.now() }),
      );
    } catch { /* storage disabled — skip return-to, still redirect */ }
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
