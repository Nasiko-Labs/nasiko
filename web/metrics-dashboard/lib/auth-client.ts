/** Browser-only: read Nasiko JWT for /api/* calls. */

const DEV_TOKEN_KEY = "nasiko-metrics-dev-token";

export function findNasikoCredentialsKey(): string | null {
  if (typeof window === "undefined") {
    return null;
  }
  for (let i = 0; i < localStorage.length; i++) {
    const key = localStorage.key(i);
    if (key?.startsWith("nasiko-credentials-")) {
      return key;
    }
  }
  return null;
}

export function getAccessTokenFromStorage(): string | null {
  if (typeof window === "undefined") {
    return null;
  }

  const devToken = sessionStorage.getItem(DEV_TOKEN_KEY);
  if (devToken?.trim()) {
    return devToken.trim();
  }

  for (let i = 0; i < localStorage.length; i++) {
    const key = localStorage.key(i);
    if (!key?.startsWith("nasiko-credentials-")) {
      continue;
    }
    try {
      const raw = localStorage.getItem(key);
      if (!raw) {
        continue;
      }
      const parsed = JSON.parse(raw) as { access_token?: string };
      if (parsed.access_token) {
        return parsed.access_token;
      }
    } catch {
      /* ignore malformed entries */
    }
  }
  return null;
}

export function setDevAccessToken(token: string): void {
  sessionStorage.setItem(DEV_TOKEN_KEY, token.trim());
}

export function clearDevAccessToken(): void {
  sessionStorage.removeItem(DEV_TOKEN_KEY);
}

export function authHeaders(token: string): HeadersInit {
  return { Authorization: `Bearer ${token}` };
}
