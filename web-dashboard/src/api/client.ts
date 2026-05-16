export type LogLevel = "INFO" | "WARNING" | "ERROR" | "";

export interface PlatformLogEntry {
  id?: string;
  timestamp: string;
  level: LogLevel;
  message: string;
  service: string;
  logger?: string;
}

export interface PlatformLogsResponse {
  logs: PlatformLogEntry[];
  total: number;
  limit: number;
  skip: number;
  level_filter?: string | null;
}

const API_BASE = import.meta.env.VITE_API_BASE ?? "";

export function getStoredToken(): string | null {
  return localStorage.getItem("nasiko_jwt");
}

export function setStoredToken(token: string): void {
  localStorage.setItem("nasiko_jwt", token);
}

export function clearStoredToken(): void {
  localStorage.removeItem("nasiko_jwt");
}

export async function login(
  accessKey: string,
  accessSecret: string
): Promise<string> {
  const res = await fetch(`${API_BASE}/auth/users/login`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      access_key: accessKey,
      access_secret: accessSecret,
    }),
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(
      (body as { detail?: string }).detail ?? `Login failed (${res.status})`
    );
  }
  const data = (await res.json()) as { token: string };
  setStoredToken(data.token);
  return data.token;
}

export async function fetchPlatformLogs(
  token: string,
  level: LogLevel,
  limit = 200
): Promise<PlatformLogsResponse> {
  const params = new URLSearchParams({ limit: String(limit) });
  if (level) {
    params.set("level", level);
  }
  const res = await fetch(`${API_BASE}/api/v1/platform/logs?${params}`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(
      (body as { detail?: string }).detail ??
        `Failed to load logs (${res.status})`
    );
  }
  return res.json() as Promise<PlatformLogsResponse>;
}
