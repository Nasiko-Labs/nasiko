// Ported from nasiko-ui-lab@1b99bdda common/src/mocks/handlers.test.ts. Local edits: the pinned seed and the
// msw/node server (./node) are set up by this file rather than `setupPinnedSeed()`; `apiFetch` is plain `fetch`; the
// persona case also checks `seedAdminSession`, the session the browser boot pins; cases for the finops envelope, the
// plain-text errors, `?mock=empty`, `server-down`, `top-traces-absent`, `/api/me`'s shape and the agents directory
// are added, since the lab checks those through page tests; `!` on indexed reads under eslint-disable comments.
/* eslint-disable @typescript-eslint/no-non-null-assertion -- the seed's shape is what the test asserts */
import { afterAll, afterEach, beforeAll, describe, expect, it, onTestFinished, vi } from "vitest";
import {
  configureMocks,
  handlerGroups,
  handlersFor,
  resetAgentsMock,
  seedAdminSession,
} from "./handlers";
import { server } from "./node";
import { generateSeed, type Seed } from "./seed";
import { ADMIN_ID, generateHarnessSeed } from "./seed-harness";

const anchor = new Date("2026-03-20T15:00:00Z");
const now = () => anchor.getTime();
const seed = generateSeed({ anchor });
const harnessSeed = generateHarnessSeed({ anchor });

beforeAll(() => {
  server.listen({ onUnhandledFrame: "error" });
});
afterAll(() => {
  server.close();
});
afterEach(() => {
  server.resetHandlers();
  resetAgentsMock();
  configureMocks({ seed, now, harnessSeed, loggedIn: true, persistLogin: null, variant: null });
});
configureMocks({ seed, now, harnessSeed, loggedIn: true });

const url = (p: string) => new URL(p, globalThis.location.origin).toString();
// eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters -- T names the body each case reads
const json = async <T>(p: string): Promise<{ status: number; body: T }> => {
  const res = await fetch(url(p));
  return { status: res.status, body: (await res.json()) as T };
};

describe("handlersFor (partial mocks)", () => {
  it("returns exactly the handlers of the named groups, in order", () => {
    expect(handlersFor([])).toEqual([]);
    expect(handlersFor(["top-traces"])).toEqual(handlerGroups["top-traces"]);
    expect(handlersFor(["providers", "spend-calendar"])).toEqual([
      ...handlerGroups.providers,
      ...handlerGroups["spend-calendar"],
    ]);
    const paths = handlersFor(["top-traces"]).map((h) => String(h.info.path));
    expect(paths).toEqual(["/api/observability/finops/top-traces"]);
  });

  it("puts an edition's handlers for the named groups in front of every core handler", () => {
    const [own] = handlerGroups.dashboard;
    const other = handlerGroups["top-traces"][0]!;
    expect(
      handlersFor(["providers", "dashboard"], { dashboard: [own!], "top-traces": [other] }),
    ).toEqual([own, ...handlerGroups.providers, ...handlerGroups.dashboard]);
  });
});

describe("auth handlers", () => {
  it("logout ends the mock session until the next login", async () => {
    expect((await fetch(url("/api/me"))).status).toBe(200);
    const out = await fetch(url("/api/auth/logout"), { method: "POST" });
    expect(out.status).toBe(204);
    expect((await fetch(url("/api/me"))).status).toBe(401);
    expect((await fetch(url("/api/observability/finops/dashboard?range=7d"))).status).toBe(401);
    // Empty credentials are rejected like the server; real ones log back in.
    expect((await fetch(url("/api/auth/login"), { method: "POST", body: "{}" })).status).toBe(401);
    expect(
      (
        await fetch(url("/api/auth/login"), {
          method: "POST",
          body: JSON.stringify({ username: "a", password: "b" }),
        })
      ).status,
    ).toBe(200);
    expect((await fetch(url("/api/me"))).status).toBe(200);
  });

  it("an unexpected aggregation error is a plain-text 500, not a crash", async () => {
    configureMocks({ seed: { ...seed, traces: null } as unknown as Seed, now, loggedIn: true });
    const res = await fetch(url("/api/observability/finops/spend-timeseries?range=7d"));
    expect(res.status).toBe(500);
    expect(res.headers.get("content-type")).toBe("text/plain");
    expect(await res.text()).toBe("internal error");
  });

  it("/api/me answers the seed admin's claims; `superuser: false` flips the flag like `?superuser=0`", async () => {
    const me = await json<{ sub: string; username: string; is_superuser: boolean }>("/api/me");
    expect(me.body).toEqual({
      sub: "5eed0000-0000-4000-8000-00000000a001",
      username: "admin",
      is_superuser: true,
    });
    configureMocks({ superuser: false });
    expect((await json<{ is_superuser: boolean }>("/api/me")).body.is_superuser).toBe(false);
  });

  it("a 401 is the AuthRejection JSON envelope; `logout-unavailable` is the fail-closed one", async () => {
    configureMocks({ loggedIn: false });
    const me = await json<{ data: null; status_code: number; message: string }>("/api/me");
    expect(me.status).toBe(401);
    expect(me.body).toEqual({
      data: null,
      status_code: 401,
      message: "missing or invalid token",
    });
    configureMocks({ loggedIn: true, variant: "logout-unavailable" });
    const out = await fetch(url("/api/auth/logout"), { method: "POST" });
    expect(out.status).toBe(401);
    expect(((await out.json()) as { message: string }).message).toBe(
      "token validation unavailable",
    );
    // Nothing was revoked.
    configureMocks({ variant: null });
    expect((await fetch(url("/api/me"))).status).toBe(200);
  });
});

describe("mock login and /api/me agree on the user (the same-account check depends on it)", () => {
  // OSS: the session is the viewer (the seed admin by default, or the persona); the browser boot pins the admin.
  it.each([null, "sam"])(
    "persona %s: login user_id and username equal the me claims",
    async (persona) => {
      onTestFinished(() => {
        configureMocks({ persona: null, session: null });
      });
      configureMocks({ persona, loggedIn: false });
      const login = await fetch(url("/api/auth/login"), {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ username: "someone", password: "x" }),
      });
      const l = (await login.json()) as { user_id: string; username: string };
      const me = (await json<{ sub: string; username: string }>("/api/me")).body;
      expect(l.user_id).toBe(me.sub);
      // auth/login.rs returns the DB row's username, not what was typed.
      expect(l.username).toBe(me.username);
    },
  );

  it("`seedAdminSession` (the browser boot's) keeps the seed admin whatever the persona", async () => {
    onTestFinished(() => {
      configureMocks({ persona: null, session: null });
    });
    configureMocks({ persona: "sam", session: seedAdminSession });
    const me = (await json<{ sub: string; username: string }>("/api/me")).body;
    expect(me.sub).toBe(ADMIN_ID);
    expect(me.username).toBe("admin");
  });
});

// Regression: ISSUE-003 (the lab's /qa 2026-09-28): a mock sign-out undid itself on the next full reload.
describe("mock session persistence (browser bootstrap hook)", () => {
  it("reports every sign-out and sign-in, so the browser can keep them across reloads", async () => {
    const persist = vi.fn();
    configureMocks({ persistLogin: persist });
    expect((await fetch(url("/api/auth/logout"), { method: "POST" })).status).toBe(204);
    expect(persist).toHaveBeenLastCalledWith(false);
    // A second logout with no session is the server's 401 and changes nothing.
    expect((await fetch(url("/api/auth/logout"), { method: "POST" })).status).toBe(401);
    expect(persist).toHaveBeenCalledTimes(1);
    const login = await fetch(url("/api/auth/login"), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: "admin", password: "x" }),
    });
    expect(login.status).toBe(200);
    expect(persist).toHaveBeenLastCalledWith(true);
  });

  it("starts signed out when the bootstrap says the last session ended", async () => {
    configureMocks({ loggedIn: false });
    expect((await fetch(url("/api/me"))).status).toBe(401);
  });
});

describe("finops handlers (the real server's quirks)", () => {
  type Envelope<T> = { data: T; status_code: number; message: string };

  it("200s are the {data, status_code, message} envelope over the seed plus the admin's harness turns", async () => {
    const d = await json<
      Envelope<{
        summary: { total_agents: number };
        agents: { agent_name: string; is_capped: boolean }[];
        kpis: { total_agents: { change_pct: number | null } };
      }>
    >("/api/observability/finops/dashboard?range=30d");
    expect(d.status).toBe(200);
    expect(d.body.status_code).toBe(200);
    expect(d.body.message).toBe("ok");
    // 20 seed agents less the deleted one, plus the admin's registered harnesses.
    expect(d.body.data.summary.total_agents).toBeGreaterThan(seed.agents.length - 1);
    expect(d.body.data.agents.some((a) => /^Codex \(/.test(a.agent_name))).toBe(true);
    // Display names on dashboard rows, never raw `seed-…` names.
    expect(d.body.data.agents.every((a) => !a.agent_name.startsWith("seed-"))).toBe(true);
    expect(d.body.data.agents.every((a) => !a.is_capped)).toBe(true);
    // The server has no previous agent count, so change_pct is null.
    expect(d.body.data.kpis.total_agents.change_pct).toBeNull();
  });

  it("timeseries carries raw names and is hourly only for 24h; the calendar and day are UTC", async () => {
    const day = await json<
      Envelope<{ bucket: string; points: { top_agent_name: string | null }[] }>
    >("/api/observability/finops/spend-timeseries?range=7d");
    expect(day.body.data.bucket).toBe("day");
    expect(day.body.data.points.find((p) => p.top_agent_name)?.top_agent_name).toMatch(/^seed-/);
    const hour = await json<Envelope<{ bucket: string }>>(
      "/api/observability/finops/spend-timeseries?range=24h",
    );
    expect(hour.body.data.bucket).toBe("hour");
    const cal = await json<Envelope<{ days: { date: string }[]; highlighted_dates: string[] }>>(
      "/api/observability/finops/spend-calendar?month=2026-03&range=7d",
    );
    expect(cal.body.data.days.length).toBe(20);
    expect(cal.body.data.highlighted_dates.length).toBe(8);
    const d = await json<Envelope<{ date: string; hours: unknown[] }>>(
      `/api/observability/finops/spend-calendar/day?date=${seed.spikeDate}`,
    );
    expect(d.body.data.date).toBe(seed.spikeDate);
    expect(d.body.data.hours).toHaveLength(24);
  });

  it("400s are plain text: a bad range, an unknown or deleted agent, a missing month", async () => {
    const range = await fetch(url("/api/observability/finops/dashboard?range=90d"));
    expect(range.status).toBe(400);
    expect(range.headers.get("content-type")).toBe("text/plain");
    expect(await range.text()).toBe("invalid range '90d'");
    const deleted = seed.agents.find((a) => a.deleted);
    const gone = await fetch(
      url(`/api/observability/finops/dashboard?range=7d&agent_id=${deleted?.id ?? ""}`),
    );
    expect(gone.status).toBe(400);
    const month = await fetch(url("/api/observability/finops/spend-calendar"));
    expect(month.status).toBe(400);
    expect(await month.text()).toBe("month is required (YYYY-MM)");
  });

  it("top-traces (proposed) pages and sorts; `top-traces-absent` is today's bare 404", async () => {
    const page = await json<Envelope<{ rows: { cost_usd: number }[]; has_more: boolean }>>(
      "/api/observability/finops/top-traces?range=30d&limit=5",
    );
    expect(page.body.data.rows).toHaveLength(5);
    expect(page.body.data.has_more).toBe(true);
    configureMocks({ variant: "top-traces-absent" });
    const absent = await fetch(url("/api/observability/finops/top-traces?range=30d"));
    expect(absent.status).toBe(404);
    expect(await absent.text()).toBe("");
  });

  it("savings is the fixed sample with coverage counted over the live agents", async () => {
    const s = await json<
      Envelope<{
        total: { basis: string };
        coverage: { agents_total: number; agents_with_compress_enabled: number };
      }>
    >("/api/observability/finops/savings");
    expect(s.body.data.total.basis).toBe("mixed");
    // The live agents (seed agents plus every developer's harnesses) span two pages of the list's 100 max.
    const page = (offset: number) =>
      json<{ compress_enabled: boolean }[]>(`/api/agents?limit=100&offset=${offset}`);
    const agents = [...(await page(0)).body, ...(await page(100)).body];
    expect(agents.length).toBeGreaterThan(100);
    expect(s.body.data.coverage.agents_total).toBe(agents.length);
    expect(s.body.data.coverage.agents_with_compress_enabled).toBe(
      agents.filter((a) => a.compress_enabled).length,
    );
  });

  it("providers groups the seed models by provider; the unpriced one has null prices", async () => {
    const p = await json<
      Envelope<
        {
          provider: string;
          models: {
            model: string;
            pricing_available: boolean;
            input_price_per_1m: number | null;
          }[];
        }[]
      >
    >("/api/llm-router/providers");
    expect(p.body.data.map((g) => g.provider).sort()).toEqual([
      "anthropic",
      "custom",
      "google",
      "openai",
    ]);
    const custom = p.body.data.find((g) => g.provider === "custom")?.models[0];
    expect(custom).toMatchObject({
      model: "custom-local",
      pricing_available: false,
      input_price_per_1m: null,
    });
    configureMocks({ variant: "router-catalog-fail" });
    expect((await fetch(url("/api/llm-router/providers"))).status).toBe(500);
  });

  it("`?mock=empty` is a fresh server: no agents, no spend", async () => {
    configureMocks({ variant: "empty" });
    expect((await json<unknown[]>("/api/agents")).body).toEqual([]);
    const d = await json<
      Envelope<{ summary: { total_cost: number; total_agents: number }; agents: unknown[] }>
    >("/api/observability/finops/dashboard?range=30d");
    expect(d.body.data.summary).toMatchObject({ total_cost: 0, total_agents: 0 });
    expect(d.body.data.agents).toEqual([]);
    const ts = await json<Envelope<{ points: unknown[] }>>(
      "/api/observability/finops/spend-timeseries?range=7d",
    );
    expect(ts.body.data.points).toEqual([]);
  });
});

describe("/health and the agents directory", () => {
  it("/health is plain-text ok; `server-down` fails it like a stopped server", async () => {
    const ok = await fetch(url("/health"));
    expect(ok.status).toBe(200);
    expect(await ok.text()).toBe("ok");
    configureMocks({ variant: "server-down" });
    await expect(fetch(url("/health"))).rejects.toThrow();
  });

  it("/api/agents is a bare array of live agents, newest first, paged, with `owner` checked as a UUID", async () => {
    const all =
      await json<{ id: string; name: string; created_at: string; owner_id: string }[]>(
        "/api/agents?limit=100",
      );
    expect(all.status).toBe(200);
    const deleted = seed.agents.find((a) => a.deleted);
    expect(all.body.some((a) => a.id === deleted?.id)).toBe(false);
    expect(all.body.map((a) => a.created_at)).toEqual(
      [...all.body.map((a) => a.created_at)].sort((a, b) => b.localeCompare(a)),
    );
    const page = await json<unknown[]>("/api/agents?limit=5&offset=5");
    expect(page.body).toHaveLength(5);
    const bad = await fetch(url("/api/agents?owner=admin"));
    expect(bad.status).toBe(400);
    expect(await bad.text()).toBe("Failed to deserialize query string: owner: UUID parsing failed");
    const mine = await json<{ owner_id: string }[]>(
      "/api/agents?owner=5eed0000-0000-4000-8000-00000000a001&limit=100",
    );
    expect(mine.body.length).toBeGreaterThan(0);
    expect(mine.body.every((a) => a.owner_id === "5eed0000-0000-4000-8000-00000000a001")).toBe(
      true,
    );
    configureMocks({ loggedIn: false });
    expect((await fetch(url("/api/agents"))).status).toBe(401);
  });
});
