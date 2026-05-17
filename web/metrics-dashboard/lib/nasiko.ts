/**
 * Server-side Nasiko API client (Route Handlers only).
 * Base URL is Kong in local dev; use http://nasiko-backend:8000 inside Docker compose.
 */

import type {
  NasikoAgentStatsResponse,
  NasikoObservabilitySessionsResponse,
  NasikoSimpleUserAgentsResponse,
} from "@/lib/types";

const DEFAULT_NASIKO_API_URL = "http://localhost:9100";

export function getNasikoApiUrl(): string {
  const base = process.env.NASIKO_API_URL?.trim() || DEFAULT_NASIKO_API_URL;
  return base.replace(/\/$/, "");
}

export class NasikoApiError extends Error {
  constructor(
    message: string,
    public readonly status: number,
    public readonly body: string,
  ) {
    super(message);
    this.name = "NasikoApiError";
  }
}

export type NasikoFetchOptions = {
  /** Full `Authorization` header value, e.g. `Bearer <token>`. */
  authorization?: string;
  /** Query params appended to the path. */
  searchParams?: Record<string, string | undefined>;
  cache?: RequestCache;
};

/**
 * GET (or custom method) against Nasiko. Path must start with `/`, e.g. `/api/v1/healthcheck`.
 */
export async function nasikoFetch<T = unknown>(
  path: string,
  options: NasikoFetchOptions & { method?: "GET" } = {},
): Promise<T> {
  const method = options.method ?? "GET";
  const base = getNasikoApiUrl();
  const normalizedPath = path.startsWith("/") ? path : `/${path}`;
  const url = new URL(`${base}${normalizedPath}`);

  if (options.searchParams) {
    for (const [key, value] of Object.entries(options.searchParams)) {
      if (value !== undefined) {
        url.searchParams.set(key, value);
      }
    }
  }

  const headers: HeadersInit = {
    Accept: "application/json",
  };
  if (options.authorization) {
    headers.Authorization = options.authorization;
  }

  const response = await fetch(url.toString(), {
    method,
    headers,
    cache: options.cache ?? "no-store",
  });

  const bodyText = await response.text();
  if (!response.ok) {
    throw new NasikoApiError(
      `Nasiko ${method} ${normalizedPath} failed`,
      response.status,
      bodyText,
    );
  }

  if (!bodyText) {
    return undefined as T;
  }

  try {
    return JSON.parse(bodyText) as T;
  } catch {
    return bodyText as T;
  }
}

/** Kong gateway health (no auth). */
export function fetchNasikoGatewayHealth(): Promise<unknown> {
  return nasikoFetch("/health");
}

/** Backend health via Kong proxy. */
export function fetchNasikoBackendHealthcheck(): Promise<unknown> {
  return nasikoFetch("/api/v1/healthcheck");
}

/** Agents visible to the authenticated user. */
export function fetchNasikoUserAgents(
  authorization: string,
): Promise<NasikoSimpleUserAgentsResponse> {
  return nasikoFetch<NasikoSimpleUserAgentsResponse>(
    "/api/v1/registry/user/agents",
    { authorization },
  );
}

/** Phoenix project stats for one agent over a time window. */
export function fetchNasikoAgentStats(
  authorization: string,
  agentId: string,
  startTimeIso: string,
): Promise<NasikoAgentStatsResponse> {
  return nasikoFetch<NasikoAgentStatsResponse>(
    `/api/v1/observability/agent/${encodeURIComponent(agentId)}/stats`,
    {
      authorization,
      searchParams: { start_time: startTimeIso },
    },
  );
}

/** Sessions across accessible agents (filter client-side by `agent_id`). */
export function fetchNasikoObservabilitySessions(
  authorization: string,
  startTimeIso: string,
): Promise<NasikoObservabilitySessionsResponse> {
  return nasikoFetch<NasikoObservabilitySessionsResponse>(
    "/api/v1/observability/session/list",
    {
      authorization,
      searchParams: { start_time: startTimeIso },
    },
  );
}

export function fetchNasikoSessionDetails(
  authorization: string,
  sessionId: string,
): Promise<unknown> {
  return nasikoFetch(
    `/api/v1/observability/session/${encodeURIComponent(sessionId)}`,
    { authorization },
  );
}

export function fetchNasikoTraceDetails(
  authorization: string,
  projectId: string,
  traceId: string,
): Promise<unknown> {
  return nasikoFetch(
    `/api/v1/observability/trace/${encodeURIComponent(projectId)}/${encodeURIComponent(traceId)}`,
    { authorization },
  );
}
