import type { LoginResponse, MetricsResponse } from "../types";

const TOKEN_KEY = "nasiko_metrics_token";

export function getStoredToken(): string | null {
  return sessionStorage.getItem(TOKEN_KEY);
}

export function storeToken(token: string): void {
  sessionStorage.setItem(TOKEN_KEY, token);
}

export function clearToken(): void {
  sessionStorage.removeItem(TOKEN_KEY);
}

function authHeaders(): HeadersInit {
  const token = getStoredToken();
  if (!token) {
    throw new Error("Not authenticated");
  }
  return {
    Authorization: `Bearer ${token}`,
    "Content-Type": "application/json",
  };
}

export async function login(
  accessKey: string,
  accessSecret: string
): Promise<string> {
  const response = await fetch("/auth/users/login", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      access_key: accessKey,
      access_secret: accessSecret,
    }),
  });

  if (!response.ok) {
    const detail = await response.text();
    throw new Error(detail || "Login failed");
  }

  const data = (await response.json()) as LoginResponse;
  const token = data.access_token ?? data.token;
  if (!token) {
    throw new Error("No token returned from auth service");
  }
  storeToken(token);
  return token;
}

export async function fetchAgentMetrics(
  hours = 24
): Promise<MetricsResponse> {
  const response = await fetch(
    `/api/v1/observability/agents/metrics?hours=${hours}`,
    { headers: authHeaders() }
  );

  if (response.status === 401) {
    clearToken();
    throw new Error("Session expired. Please sign in again.");
  }

  if (!response.ok) {
    const detail = await response.text();
    throw new Error(detail || "Failed to load metrics");
  }

  return response.json() as Promise<MetricsResponse>;
}
