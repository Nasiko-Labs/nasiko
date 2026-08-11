// Auth service — server handles authentication via HttpOnly cookie.
// Frontend fetches user info from /api/me.

// Per-tab cache of the /api/me answer. This is an MPA: without it every
// navigation starts with no identity, so the shell renders once without the
// user and again once the fetch lands — the visible sidebar rebuild. Cleared
// on logout and on any 401, and it holds nothing the page couldn't already
// read from /api/me.
const CACHE_KEY = 'nasiko-current-user';

function readCache() {
  try {
    const raw = sessionStorage.getItem(CACHE_KEY);
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}

function writeCache(user) {
  try {
    if (user) sessionStorage.setItem(CACHE_KEY, JSON.stringify(user));
    else sessionStorage.removeItem(CACHE_KEY);
  } catch { /* private mode / quota — in-memory cache still applies */ }
}

/// Drops every per-tab shell cache. The nav trees are role-derived, so they
/// must not survive a sign-out into the next user's session.
export function clearShellCache() {
  try {
    sessionStorage.removeItem(CACHE_KEY);
    sessionStorage.removeItem('app-header-nav');
    Object.keys(sessionStorage)
      .filter((k) => k.startsWith('app-module-nav:'))
      .forEach((k) => sessionStorage.removeItem(k));
  } catch { /* nothing to clear */ }
}

let _cachedUser = readCache();

class AuthService {
  getCurrentUser() {
    return _cachedUser?.name || null;
  }

  // Caller's user UUID (JWT `sub`). Truthful only after fetchCurrentUser().
  getCurrentUserId() {
    return _cachedUser?.id || null;
  }

  // Truthful only after fetchCurrentUser().
  isSuperuser() {
    return _cachedUser?.is_superuser === true;
  }

  // Truthful only after fetchCurrentUser() has resolved (app-header awaits it
  // before rendering). The cookie is HttpOnly, so a server round-trip is the
  // only way to learn auth state — there is nothing local to check.
  isAuthenticated() {
    return _cachedUser !== null;
  }

  getUsers() {
    return _cachedUser ? [{ username: _cachedUser.name, email: _cachedUser.email }] : [];
  }

  async fetchCurrentUser() {
    if (_cachedUser) return _cachedUser;
    try {
      const res = await fetch('/api/me');
      if (res.status === 401 || res.status === 403) {
        // Token is invalid/expired — clear it and redirect to login
        _cachedUser = null;
        writeCache(null);
        clearShellCache();
        await fetch('/api/auth/logout', { method: 'POST' }).catch(() => {});
        if (!window.location.pathname.startsWith('/login')) {
          window.location.href = '/login.html';
        }
        return null;
      }
      if (!res.ok) return null;
      const claims = await res.json();
      _cachedUser = {
        id: claims.sub || null,
        name: claims.username || claims.sub || 'User',
        email: claims.email || '',
        is_superuser: claims.is_superuser === true,
      };
      writeCache(_cachedUser);
      return _cachedUser;
    } catch {
      return null;
    }
  }

  // Sign out, clearing BOTH identities of the multi-tenant model:
  //   1. the management session — the access_token cookie on THIS (dashboard/BFF)
  //      origin, cleared by a same-origin POST /api/auth/logout;
  //   2. the workspace control-plane session — a separate host-only cookie on the
  //      cross-origin CP (window.nasikoConfig.apiBase). nasiko.dev and
  //      <sub>.nasiko.dev are same-site, so the Strict CP cookie rides a
  //      credentialed cross-origin POST, and the CP (which allow-lists the BFF
  //      origin for credentialed CORS) returns the clearing Set-Cookie.
  // Single-tenant: apiBase is empty, so only the same-origin logout runs — the
  // CP's own logout — which is exactly right.
  //
  // Both calls are awaited (with keepalive as a belt-and-braces against the
  // navigation cancelling them) BEFORE navigating, and _cachedUser is cleared,
  // so the old broken race — navigate first, get silently re-authenticated by a
  // still-valid cookie — can't happen.
  //
  // V1 scope: this is a FULL sign-out (both identities). docs/MULTITENANT.md §10E
  // describes a current-workspace-only default plus a distinct "Sign out of
  // Nasiko" — but that split is a multi-workspace affordance: with V1's single
  // domain-derived workspace, a workspace-only sign-out would just land you on a
  // dashboard that immediately re-enters via /api/enter. Deferred with
  // multi-workspace; full sign-out is the correct single-workspace behavior.
  async logout() {
    _cachedUser = null;
    // The per-tab shell caches (identity + role-derived nav trees) must not
    // survive a sign-out into the next user's session.
    clearShellCache();
    const base = window.nasikoConfig?.apiBase || '';
    const calls = [
      fetch('/api/auth/logout', { method: 'POST', credentials: 'same-origin', keepalive: true }).catch(() => {}),
    ];
    if (base) {
      calls.push(
        fetch(`${base}/api/auth/logout`, { method: 'POST', credentials: 'include', keepalive: true }).catch(() => {}),
      );
    }
    await Promise.all(calls);
    window.location.href = '/login.html';
  }

  // Back-compat shim: the reused multi-account UI calls this with a username, but
  // the backend is single-session, so any sign-out is a full logout.
  removeUserSession(_username) {
    this.logout();
  }
}

export const authService = new AuthService();
export default authService;
