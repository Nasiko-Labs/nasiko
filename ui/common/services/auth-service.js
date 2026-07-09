// Auth service — server handles authentication via HttpOnly cookie.
// Frontend fetches user info from /api/me.

let _cachedUser = null;

class AuthService {
  getCurrentUser() {
    return _cachedUser?.name || null;
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
        name: claims.username || claims.sub || 'User',
        email: claims.email || '',
      };
      return _cachedUser;
    } catch {
      return null;
    }
  }

  removeUserSession(_username) {
    fetch('/api/auth/logout', { method: 'POST' }).finally(() => {
      window.location.href = '/login.html';
    });
  }
}

export const authService = new AuthService();
export default authService;
