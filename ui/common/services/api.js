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
    // A 401 from the workspace control plane means its session cookie is missing
    // or expired. Bootstrap it via redirect-and-return (docs/MULTITENANT.md §4.4):
    // hand the browser to the BFF's /api/enter, which sends it into the
    // workspace's own SSO (silent — the IdP session is live), the CP sets its
    // host-only cookie, and we land back here.
    //
    // Same-origin (single-tenant, apiBase empty) has no separate CP to bootstrap,
    // so it just re-auths at /login.html as before.
    let restart = '/login.html';
    if (base) {
      // Remember the deep link: OIDC returns straight to it via /api/enter's
      // redirect param; GitHub returns to `/`, where the injected restore
      // snippet reads this and replaces to the deep link.
      try {
        sessionStorage.setItem(
          'nasiko:returnTo',
          JSON.stringify({ p: location.pathname + location.search, t: Date.now() }),
        );
      } catch { /* storage disabled — still redirect */ }
      // Loop guard: if an entry attempt just happened and we're STILL 401 (e.g.
      // admission rejected the account), fall back to a full BFF re-auth instead
      // of bouncing through /api/enter forever.
      let lastEnter = 0;
      try { lastEnter = +(sessionStorage.getItem('nasiko:enterAt') || 0); } catch { /* ignore */ }
      if (Date.now() - lastEnter < 15000) {
        try { sessionStorage.removeItem('nasiko:enterAt'); } catch { /* ignore */ }
      } else {
        try { sessionStorage.setItem('nasiko:enterAt', String(Date.now())); } catch { /* ignore */ }
        restart = '/api/enter?return_to=' + encodeURIComponent(location.pathname + location.search);
      }
    }
    window.location.href = restart;
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
