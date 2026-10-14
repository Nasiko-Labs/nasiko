import { http, HttpResponse, type RequestHandler } from "msw";

/** A pure response the HTTP and MSW adapters both serve. */
export interface SyntheticResponse {
  readonly status: number;
  readonly headers: Readonly<Record<string, string>>;
  readonly body: unknown;
}

/**
 * Deterministic synthetic control plane (contract §9 fixtures): two
 * workspaces, two users, per-workspace data that never overlaps, and the
 * awkward cases — slow, partial, unpriced, huge totals, revoked sessions.
 * Used by vitest (msw/node) and by the browser harness (msw/browser).
 *
 * Authorization model mirrors the real CP: a request carries either a
 * session cookie (`access_token=<user>@<workspace>`) or a delegated bearer
 * (`Bearer delegated:<user>@<workspace>;<scope,scope>`). Anything else → 401.
 * A workspace never answers for another workspace's credential → 403.
 */
export interface SyntheticWorld {
  readonly workspaces: Record<
    string,
    {
      readonly origin: string;
      readonly agents: readonly string[];
      readonly members: readonly string[];
    }
  >;
  revoked: Set<string>;
  slowMs: number;
}

export function createWorld(): SyntheticWorld {
  return {
    workspaces: {
      "ws-a": {
        origin: "http://ws-a.test",
        agents: ["alpha-agent", "beta-agent", "gamma-agent"],
        members: ["user-1"],
      },
      "ws-b": {
        origin: "http://ws-b.test",
        agents: ["delta-agent", "epsilon-agent"],
        members: ["user-2"],
      },
    },
    revoked: new Set(),
    slowMs: 0,
  };
}

interface Caller {
  readonly user: string;
  readonly workspace: string;
  readonly scopes: readonly string[] | "session";
}

const json = (
  status: number,
  body: unknown,
  headers: Record<string, string> = {},
): SyntheticResponse => ({
  status,
  headers: { "content-type": "application/json", ...headers },
  body,
});
const envelope = (status: number, message: string): SyntheticResponse =>
  json(status, { data: null, status_code: status, message });

function authenticate(
  world: SyntheticWorld,
  headers: Headers,
  workspace: string,
): Caller | SyntheticResponse {
  const auth = headers.get("authorization");
  const cookie = headers.get("cookie") ?? "";
  let caller: Caller | undefined;
  if (auth?.startsWith("Bearer delegated:")) {
    const [, rest] = auth.split("delegated:");
    const [who, scopes] = (rest ?? "").split(";");
    const [user, ws] = (who ?? "").split("@");
    if (user && ws)
      caller = { user, workspace: ws, scopes: (scopes ?? "").split(",").filter(Boolean) };
  } else {
    const m = /access_token=([^;]+)/.exec(cookie);
    const [user, ws] = (m?.[1] ?? "").split("@");
    if (user && ws) caller = { user, workspace: ws, scopes: "session" };
  }
  if (!caller) return envelope(401, "missing or invalid token");
  if (world.revoked.has(caller.user)) return envelope(401, "invalid token");
  if (
    caller.workspace !== workspace ||
    !world.workspaces[workspace]?.members.includes(caller.user)
  ) {
    return envelope(403, "forbidden");
  }
  return caller;
}

function needScope(caller: Caller, scope: string): SyntheticResponse | undefined {
  if (caller.scopes === "session") return undefined;
  if (!caller.scopes.includes(scope)) return envelope(403, "scope");
  return undefined;
}

const SCOPE_CEILING = [
  "me:read",
  "usage:read",
  "finops:read",
  "agents:read",
  "surfaces:read",
] as const;

/**
 * Pure request → response for one workspace of the world. Mirrors the real CP's
 * envelope and auth rules, plus the two Phase 2 endpoints the design adds:
 * `GET /api/me` (Claims) and `POST /api/auth/delegate` (ADR-0001a §2.1).
 */
export async function respond(
  world: SyntheticWorld,
  wsId: string,
  method: string,
  url: URL,
  headers: Headers,
  body: unknown,
): Promise<SyntheticResponse> {
  const ws = world.workspaces[wsId];
  if (!ws) return envelope(404, "no such workspace");
  if (world.slowMs) await new Promise((r) => setTimeout(r, world.slowMs));
  const path = url.pathname;
  if (method === "POST" && path === "/api/auth/login") {
    // auth/login.rs `login`: any member of this workspace signs in (the world has no passwords); the session is the
    // HttpOnly cookie, as on the real CP. A new sign-in is a new session, so an earlier sign-out no longer applies.
    const user = (body as { username?: unknown } | undefined)?.username;
    if (typeof user !== "string" || !ws.members.includes(user))
      return envelope(401, "invalid credentials");
    world.revoked.delete(user);
    const token = `${user}@${wsId}`;
    return json(
      200,
      { token, user_id: user, username: user, is_superuser: false, expires_in: 3600 },
      { "set-cookie": `access_token=${token}; Path=/; HttpOnly; SameSite=Lax` },
    );
  }
  const c = authenticate(world, headers, wsId);
  if (!("user" in c)) return c;
  const rid = { "x-request-id": `rid-${seeded(url.toString()).toString(16)}` };

  if (method === "GET" && path === "/api/me") {
    const d = needScope(c, "me:read");
    return d ?? json(200, { sub: c.user, username: c.user, is_superuser: false }, rid);
  }
  if (method === "POST" && path === "/api/auth/delegate") {
    // Only a session may delegate; a delegated token cannot chain.
    if (c.scopes !== "session") return envelope(403, "delegated tokens cannot delegate");
    const req = (body ?? {}) as { scopes?: unknown; ttl_secs?: unknown };
    const asked = Array.isArray(req.scopes)
      ? req.scopes.filter((x): x is string => typeof x === "string")
      : [];
    if (asked.length === 0 || asked.some((x) => !(SCOPE_CEILING as readonly string[]).includes(x)))
      return envelope(400, "unknown scope");
    const ttl = Math.min(typeof req.ttl_secs === "number" ? req.ttl_secs : 60, 60);
    return json(
      200,
      {
        token: `delegated:${c.user}@${wsId};${asked.join(",")}`,
        expires_in: ttl,
        scopes: asked,
        aud: ws.origin,
      },
      rid,
    );
  }
  if (method === "POST" && path === "/api/auth/logout") {
    // auth/login.rs `logout`: the session ends; every later request with it is a 401 (the world's revocation).
    if (c.scopes !== "session") return envelope(403, "delegated tokens cannot sign out");
    world.revoked.add(c.user);
    return { status: 204, headers: rid, body: null };
  }
  if (method !== "GET") return envelope(405, "read-only");
  const r = (scope: string, build: () => unknown): SyntheticResponse =>
    needScope(c, scope) ?? json(200, build(), rid);
  const q = url.searchParams;
  switch (path) {
    case "/api/usage/summary":
      return r("usage:read", () => ({
        request_count: 100 + ws.agents.length,
        total_tokens: 5000,
        total_input_tokens: 3000,
        total_output_tokens: 2000,
        total_cost_usd: money(wsId, 0),
        avg_latency_ms: 123.4,
        period_days: 30,
      }));
    case "/api/usage/history": {
      const days = Number(q.get("days") ?? "7");
      return r("usage:read", () =>
        Array.from({ length: days }, (_, i) => ({
          date: `2026-09-${String((i % 28) + 1).padStart(2, "0")}`,
          request_count: i + 1,
          total_tokens: (i + 1) * 10,
          total_cost_usd: money(wsId, i),
        })),
      );
    }
    case "/api/usage/by-agent": {
      const needle = (q.get("q") ?? "").toLowerCase();
      const limit = Number(q.get("limit") ?? "50");
      const offset = Number(q.get("offset") ?? "0");
      const all = Array.from(
        { length: 2000 },
        (_, i) => `${ws.agents[i % ws.agents.length]}-${i}`,
      ).filter((n) => n.includes(needle));
      return r("usage:read", () => ({
        data: all.slice(offset, offset + limit).map((agent_name, i) => ({
          agent_name,
          request_count: i + 1,
          total_tokens: 10 * (i + 1),
          total_cost_usd: money(wsId, offset + i),
          avg_latency_ms: i % 3 === 0 ? null : 100 + i,
        })),
        total: all.length,
      }));
    }
    case "/api/usage/by-model":
      return r("usage:read", () => ({
        data: [
          {
            provider: "bedrock",
            model: `${wsId}-model`,
            request_count: 1,
            total_tokens: 10,
            total_cost_usd: money(wsId, 7),
            avg_latency_ms: 50,
          },
        ],
        total: 1,
      }));
    case "/api/observability/finops/dashboard":
      return r("finops:read", () => ({
        data: {
          kpi: {
            total_spend_usd: { current: money(wsId, 1), previous: money(wsId, 2), change_pct: 3 },
          },
          attributions: [],
        },
        status_code: 200,
        message: "ok",
      }));
    case "/api/observability/finops/spend-timeseries":
      return r("finops:read", () => ({
        data: {
          bucket: "day",
          points: Array.from({ length: 7 }, (_, i) => ({
            bucket_start: `2026-09-2${i}T00:00:00Z`,
            spend_usd: money(wsId, 10 + i),
            operations: i,
            tool_calls: 0,
            top_agent_name: ws.agents[0] ?? null,
            top_agent_spend_usd: i === 3 ? null : money(wsId, 20 + i),
            p50_latency_ms: null,
            p95_latency_ms: null,
            p99_latency_ms: null,
          })),
        },
        status_code: 200,
        message: "ok",
      }));
    case "/api/observability/finops/spend-calendar":
      return r("finops:read", () => ({
        data: {
          days: Array.from({ length: 30 }, (_, i) => ({
            date: `2026-09-${String(i + 1).padStart(2, "0")}`,
            spend_usd: money(wsId, 30 + i),
            operations: i,
            intensity: (i % 10) / 10,
          })),
          highlighted_dates: [],
        },
        status_code: 200,
        message: "ok",
      }));
    case "/api/observability/finops/spend-calendar/day":
      return r("finops:read", () => ({
        data: {
          hours: Array.from({ length: 24 }, (_, h) => ({
            hour: h,
            spend_usd: money(wsId, 60 + h),
            top_agents: [{ agent_name: ws.agents[0] ?? "a", spend_usd: money(wsId, 90 + h) }],
            others_spend_usd: 0,
          })),
          top_agents: [{ agent_name: ws.agents[0] ?? "a", spend_usd: money(wsId, 99) }],
          others_spend_usd: 0,
          avg_hourly_spend_usd: money(wsId, 98),
        },
        status_code: 200,
        message: "ok",
      }));
    case "/api/observability/finops/attributions":
      return r("finops:read", () => ({
        data: {
          view: "agent",
          rows: ws.agents.map((agent_name, i) => ({
            agent_id: `${wsId}-${i}`,
            agent_name,
            total_cost: money(wsId, 100 + i),
            operations: 10 + i,
            is_capped: false,
            avg_cost_per_operation: money(wsId, 200 + i),
            prompt_tokens: 100,
            completion_tokens: 50,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            total_tokens: 150,
            avg_latency_ms: null,
            avg_latency_p95_ms: null,
            avg_latency_p99_ms: null,
            tool_call_count: 0,
            version: null,
          })),
        },
        status_code: 200,
        message: "ok",
      }));
    default:
      return envelope(404, "not found");
  }
}

const seeded = (s: string): number => {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619) >>> 0;
  return h;
};
const money = (ws: string, i: number): number =>
  Number((((seeded(`${ws}${i}`) % 100000) / 100) * 0.37).toFixed(4));

export function syntheticHandlers(world: SyntheticWorld): RequestHandler[] {
  const out: RequestHandler[] = [];
  for (const [wsId, ws] of Object.entries(world.workspaces)) {
    out.push(
      http.all(`${ws.origin}/api/*`, async ({ request }) => {
        const body =
          request.method === "POST" ? await request.json().catch(() => undefined) : undefined;
        const r = await respond(
          world,
          wsId,
          request.method,
          new URL(request.url),
          request.headers,
          body,
        );
        return HttpResponse.json(r.body as Record<string, unknown>, {
          status: r.status,
          headers: r.headers,
        });
      }),
    );
  }
  return out;
}

/** Credential helpers for tests and the harness. */
export const sessionCookie = (user: string, workspace: string): string =>
  `access_token=${user}@${workspace}`;
export const delegatedBearer = (
  user: string,
  workspace: string,
  scopes: readonly string[],
): string => `Bearer delegated:${user}@${workspace};${scopes.join(",")}`;
