// Ported from nasiko-ui-lab@1b99bdda common/src/lib/api/auth.lockout.test.ts. Local edits: the copy
// assertion (`copy.login.locked`) stays with the app's strings; `safeRedirect`, the login/logout/me
// calls and detect.ts rules are covered here too; msw server set up locally; Prettier formatting.

import { http, HttpResponse } from "msw";
import { setupServer } from "msw/node";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { fetchMe, lockedFor, login, logout, meQueryOptions, safeRedirect } from "./auth";
import { configureProductApi, ProductApiError } from "./client";
import { errorCode, isDeadSession401, isEndpointAbsent, isUnauthorized } from "./detect";

const ORIGIN = "http://product.test";
const server = setupServer();
beforeAll(() => {
  server.listen({ onUnhandledFrame: "error" });
  configureProductApi({ origin: ORIGIN });
});
afterEach(() => {
  server.resetHandlers();
});
afterAll(() => {
  server.close();
  configureProductApi({});
});

describe("lockedFor", () => {
  it("is null for anything but account_locked (the plain rate limit); otherwise whole minutes, at least 1, else 15", () => {
    for (const body of [
      null,
      undefined,
      "Too Many Requests",
      {},
      { code: "rate_limited", retry_after_secs: 60 },
    ])
      expect(lockedFor(body)).toBeNull();
    // Rounds the wait up to whole minutes, at least 1; without a usable wait, the server's 15-minute lock.
    expect(lockedFor({ code: "account_locked", retry_after_secs: 840 })).toBe(14);
    expect(lockedFor({ code: "account_locked", retry_after_secs: 841 })).toBe(15);
    expect(lockedFor({ code: "account_locked", retry_after_secs: 30 })).toBe(1);
    for (const retry_after_secs of [undefined, 0, -5, "soon", Number.POSITIVE_INFINITY])
      expect(lockedFor({ code: "account_locked", retry_after_secs })).toBe(15);
  });
});

describe("safeRedirect", () => {
  it("accepts same-app relative paths only", () => {
    expect(safeRedirect("/tokenops?range=7d")).toBe("/tokenops?range=7d");
    for (const bad of [
      undefined,
      42,
      "https://evil.example",
      "//evil.example",
      "/a\\b",
      "/login",
      "/login?redirect=/x",
      "/login/again",
      "relative",
    ])
      expect(safeRedirect(bad)).toBe("/");
    expect(safeRedirect("/loginish")).toBe("/loginish");
    expect(safeRedirect(null, "/home")).toBe("/home");
  });
});

describe("login / logout / me", () => {
  it("login posts JSON credentials and returns the result; a 429 keeps the lockout body", async () => {
    let body: unknown;
    server.use(
      http.post(`${ORIGIN}/api/auth/login`, async ({ request }) => {
        body = await request.json();
        return HttpResponse.json({
          token: "t",
          user_id: "u1",
          username: "admin",
          is_superuser: true,
          expires_in: 3600,
        });
      }),
    );
    const r = await login("admin", "changeme");
    expect(body).toEqual({ username: "admin", password: "changeme" });
    expect(r.username).toBe("admin");
    server.use(
      http.post(`${ORIGIN}/api/auth/login`, () =>
        HttpResponse.json({ code: "account_locked", retry_after_secs: 900 }, { status: 429 }),
      ),
    );
    const err = (await login("admin", "nope").catch((e: unknown) => e)) as ProductApiError;
    expect(err.status).toBe(429);
    expect(lockedFor(err.body)).toBe(15);
    expect(errorCode(err)).toBe("account_locked");
  });

  it("logout posts and me reads the claims; meQueryOptions is scoped and never retries", async () => {
    server.use(
      http.post(`${ORIGIN}/api/auth/logout`, () => new HttpResponse(null, { status: 204 })),
      http.get(`${ORIGIN}/api/me`, () =>
        HttpResponse.json({ sub: "u1", username: "admin", is_superuser: false }),
      ),
    );
    expect(await logout()).toBeNull();
    expect(await fetchMe()).toEqual({ sub: "u1", username: "admin", is_superuser: false });
    const o = meQueryOptions({ workspaceId: "ws-a", epoch: 3 });
    expect(o.queryKey).toEqual(["nasiko", "product", "ws-a", 3, "me"]);
    expect(o.retry).toBe(false);
    expect(await o.queryFn({ signal: new AbortController().signal })).toMatchObject({ sub: "u1" });
  });
});

describe("detect", () => {
  const e = (status: number, body: unknown) => new ProductApiError(status, body, "/p", "m");
  it("isEndpointAbsent: a bare or router-envelope 404, never a coded or named one", () => {
    expect(isEndpointAbsent(e(404, null))).toBe(true);
    expect(isEndpointAbsent(e(404, ""))).toBe(true);
    expect(isEndpointAbsent(e(404, "Not Found"))).toBe(true);
    expect(
      isEndpointAbsent(e(404, { data: null, message: "no API route matches", status_code: 404 })),
    ).toBe(true);
    expect(isEndpointAbsent(e(404, "agent not found"))).toBe(false);
    expect(isEndpointAbsent(e(404, { error: "x", code: "not_visible" }))).toBe(false);
    expect(isEndpointAbsent(e(500, null))).toBe(false);
    expect(isEndpointAbsent(new Error("x"))).toBe(false);
  });
  it("isDeadSession401 and isUnauthorized", () => {
    expect(isDeadSession401(e(401, { message: "token revoked" }))).toBe(true);
    expect(isDeadSession401(e(401, "invalid token"))).toBe(true);
    expect(isDeadSession401(e(401, { message: "token validation unavailable" }))).toBe(false);
    expect(isDeadSession401(e(403, { message: "token revoked" }))).toBe(false);
    expect(isUnauthorized(e(401, null))).toBe(true);
    expect(isUnauthorized(e(403, null))).toBe(false);
    expect(isUnauthorized(new Error("401"))).toBe(false);
  });
});
