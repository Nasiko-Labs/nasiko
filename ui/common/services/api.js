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

// Once any 401 in THIS document has begun a redirect, every other in-flight 401
// must skip the guard. A page fires several control-plane requests at once, and
// when the CP cookie is missing they ALL 401 before the navigation actually
// happens — so without this, each 401 continuation would read+increment the
// persistent attempt counter, and a single page load with N requests could burn
// all N retries and jump straight to the dead-end. Module-scoped, so it resets to
// false on the next document load (each enter round-trip is a fresh navigation),
// giving exactly one increment per round-trip.
let enterRedirectStarted = false;

/// Low-level: performs the request and handles 401 by redirecting to the
/// login page. Returns the raw Response for callers that stream, read text,
/// or branch on status themselves.
export async function apiFetch(path, opts = {}) {
  const base = apiBase();
  // Cross-origin CP calls ride the CP's host-only session cookie.
  const withCreds = base ? { credentials: "include", ...opts } : opts;
  const res = await fetch(`${base}/api${path}`, withCreds);

  // Any CP response that ISN'T 401 means the workspace session was accepted —
  // clear the enter-attempt counter so a later, transient 401 gets a fresh set
  // of retries rather than inheriting stale failures from earlier in the session.
  if (base && res.status !== 401) {
    try { sessionStorage.removeItem('nasiko:enterTries'); } catch { /* ignore */ }
  }

  if (res.status === 401 && !window.location.pathname.startsWith('/login')) {
    // Once-per-document: only the FIRST 401 drives the redirect. Every other
    // concurrent 401 just hangs (a navigation is already underway), so the
    // attempt counter below is incremented once per document — i.e. once per
    // enter round-trip — never once per in-flight request.
    if (enterRedirectStarted) {
      return new Promise(() => {}); // redirect already underway; never settle
    }
    enterRedirectStarted = true;

    // A 401 from the workspace control plane means its session cookie is missing
    // or expired. Bootstrap it via redirect-and-return (docs/MULTITENANT.md §4.4):
    // hand the browser to the BFF's /api/enter, which sends it into the
    // workspace's own SSO (silent — the IdP session is live), the CP sets its
    // host-only cookie, and we land back here.
    //
    // Same-origin (single-tenant, apiBase empty) has no separate CP to bootstrap,
    // so it just re-auths at /login.html as before.
    if (!base) {
      window.location.href = '/login.html';
      return new Promise(() => {}); // page is navigating away; never settle
    }

    // Remember the deep link: OIDC returns straight to it via /api/enter's
    // redirect param; GitHub returns to `/`, where the injected restore
    // snippet reads this and replaces to the deep link.
    try {
      sessionStorage.setItem(
        'nasiko:returnTo',
        JSON.stringify({ p: location.pathname + location.search, t: Date.now() }),
      );
    } catch { /* storage disabled — still redirect */ }

    // Loop guard — ATTEMPT-based, not time-based. Count CONSECUTIVE /api/enter
    // round-trips that come back STILL 401; after MAX_ENTER_TRIES, dead-end on an
    // explicit error instead of bouncing forever. Counting attempts (vs the old
    // 15s window) is robust regardless of latency: a slow-but-succeeding enter
    // clears the counter on its first non-401 response above, while a genuinely
    // broken one (unregistered relay URI, admission reject, dead IdP session)
    // stops after N tries even if each round-trip is slower than any time window.
    const MAX_ENTER_TRIES = 2;
    let tries = 0;
    try { tries = +(sessionStorage.getItem('nasiko:enterTries') || 0); } catch { /* ignore */ }
    if (tries >= MAX_ENTER_TRIES) {
      try { sessionStorage.removeItem('nasiko:enterTries'); } catch { /* ignore */ }
      // Couldn't establish a workspace session after repeated tries — a
      // diagnosable dead-end beats an invisible spin. login.html shows #error.
      window.location.href = '/login.html#error=workspace_session_failed';
      return new Promise(() => {}); // page is navigating away; never settle
    }
    try { sessionStorage.setItem('nasiko:enterTries', String(tries + 1)); } catch { /* ignore */ }
    window.location.href = '/api/enter?return_to=' + encodeURIComponent(location.pathname + location.search);
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
