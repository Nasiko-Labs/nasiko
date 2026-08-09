// Auth service — server handles authentication via HttpOnly cookie.
// Frontend fetches user info from /api/me.

let _cachedUser = null;

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
  async logout() {
    _cachedUser = null;
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
