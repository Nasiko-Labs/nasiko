// Ported from nasiko-ui-lab@1b99bdda common/src/lib/api/client.test.ts. Local edits: msw server set up
// here (the lab's shared `@/test/setup`); the client is pointed at a test origin through
// `configureProductApi`; `ApiError` is `ProductApiError`; Prettier formatting.

/** The API client: ProductApiError message extraction, abort/envelope handling, the deadline and Retry-After. */
import { delay, http, HttpResponse } from "msw";
import { setupServer } from "msw/node";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import * as z from "zod";
import { apiData, apiFetch, configureProductApi, ProductApiError, withQuery } from "./client";

const ORIGIN = "http://product.test";
const server = setupServer();
beforeAll(() => {
  server.listen({ onUnhandledFrame: "error" });
  configureProductApi({ origin: ORIGIN });
});
afterEach(() => {
  server.resetHandlers();
  vi.restoreAllMocks();
});
afterAll(() => {
  server.close();
  configureProductApi({});
});

describe("ProductApiError.serverMessage", () => {
  it("prefers trimmed text, then JSON message, then JSON error, else null", () => {
    expect(new ProductApiError(400, "  bad agent  ", "/p", "m").serverMessage).toBe("bad agent");
    expect(new ProductApiError(400, "   ", "/p", "m").serverMessage).toBeNull();
    expect(
      new ProductApiError(500, { message: "boom", error: "ignored" }, "/p", "m").serverMessage,
    ).toBe("boom");
    expect(
      new ProductApiError(401, { error: "invalid credentials" }, "/p", "m").serverMessage,
    ).toBe("invalid credentials");
    expect(
      new ProductApiError(500, { error: { code: -32600, message: "rpc" } }, "/p", "m")
        .serverMessage,
    ).toBe("rpc");
    expect(new ProductApiError(500, { message: 42 }, "/p", "m").serverMessage).toBeNull();
    expect(new ProductApiError(500, null, "/p", "m").serverMessage).toBeNull();
  });
});

describe("apiFetch / apiData", () => {
  it("rethrows an abort as-is; any other fetch failure becomes a 502 network error", async () => {
    // Stub fetch: under jsdom, Node's fetch rejects with a DOMException from another realm,
    // so this exercises the branch as a browser would hit it.
    vi.spyOn(globalThis, "fetch").mockRejectedValueOnce(new DOMException("aborted", "AbortError"));
    const err = await apiFetch("/api/slow").catch((e: unknown) => e);
    expect(err).not.toBeInstanceOf(ProductApiError);
    expect((err as Error).name).toBe("AbortError");

    // Any other fetch failure is a 502 "network error" (server unreachable).
    vi.spyOn(globalThis, "fetch").mockRejectedValueOnce(new TypeError("Failed to fetch"));
    const net = await apiFetch("/api/x", { method: "POST" }).catch((e: unknown) => e);
    expect(net).toBeInstanceOf(ProductApiError);
    expect((net as ProductApiError).isServerUnreachable).toBe(true);
    expect((net as ProductApiError).message).toBe("POST /api/x → network error");
  });

  it("sends same-origin credentials and an Accept header against the configured origin", async () => {
    let seen: Request | undefined;
    server.use(
      http.get(`${ORIGIN}/api/me`, ({ request }) => {
        seen = request;
        return HttpResponse.json({ sub: "u1" });
      }),
    );
    expect(await apiFetch<{ sub: string }>("/api/me")).toEqual({ sub: "u1" });
    expect(seen?.url).toBe(`${ORIGIN}/api/me`);
    expect(seen?.headers.get("accept")).toBe("application/json");
    expect(seen?.credentials).toBe("same-origin");
  });

  it("uses the configured fetchImpl when one is set", async () => {
    const fetchImpl = vi.fn<typeof fetch>().mockResolvedValue(HttpResponse.json({ ok: true }));
    configureProductApi({ origin: ORIGIN, fetchImpl });
    try {
      expect(await apiFetch("/api/ping")).toEqual({ ok: true });
      expect(fetchImpl).toHaveBeenCalledTimes(1);
    } finally {
      configureProductApi({ origin: ORIGIN });
    }
  });

  it("keeps a plain-text error body and the status", async () => {
    server.use(
      http.get(`${ORIGIN}/api/thing`, () => new HttpResponse("agent not found", { status: 404 })),
    );
    const err = (await apiFetch("/api/thing").catch((e: unknown) => e)) as ProductApiError;
    expect(err.status).toBe(404);
    expect(err.body).toBe("agent not found");
    expect(err.path).toBe("/api/thing");
    expect(err.message).toBe("GET /api/thing → 404");
  });

  it.each([
    ["an empty 204", () => new HttpResponse(null, { status: 204 })],
    ["an HTML fallback page", () => new HttpResponse("<!doctype html>", { status: 200 })],
    ["a null data field", () => HttpResponse.json({ data: null, status_code: 200 })],
  ])("apiData fails loudly on %s", async (_, reply) => {
    server.use(http.get(`${ORIGIN}/api/thing`, reply));
    const err = await apiData("/api/thing").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ProductApiError);
    expect((err as ProductApiError).message).toMatch(/no data envelope/);
  });

  it("apiData unwraps the envelope", async () => {
    server.use(
      http.get(`${ORIGIN}/api/thing`, () =>
        HttpResponse.json({ data: { rows: [1] }, status_code: 200, message: "ok" }),
      ),
    );
    expect(await apiData("/api/thing")).toEqual({ rows: [1] });
  });

  it("a body that fails its schema is a status-200 ProductApiError, never retryable", async () => {
    vi.spyOn(console, "error").mockImplementation(() => undefined);
    server.use(
      http.get(`${ORIGIN}/api/thing`, () =>
        HttpResponse.json({ data: { total_cost: "lots" }, status_code: 200 }),
      ),
    );
    const schema = z.looseObject({ total_cost: z.number().optional() });
    const err = (await apiData<{ total_cost: number }>("/api/thing", { schema }).catch(
      (e: unknown) => e,
    )) as ProductApiError;
    expect(err).toBeInstanceOf(ProductApiError);
    expect(err.status).toBe(200);
    expect(err.isRetryable).toBe(false);
    expect(err.message).toMatch(/unexpected response \(total_cost:/);
    expect(err.body).toEqual({ total_cost: "lots" });
    // A passing body is returned as sent, never transformed.
    server.use(
      http.get(`${ORIGIN}/api/thing`, () =>
        HttpResponse.json({ data: { total_cost: 1, extra: true }, status_code: 200 }),
      ),
    );
    expect(await apiData<{ total_cost: number }>("/api/thing", { schema })).toEqual({
      total_cost: 1,
      extra: true,
    });
  });
});

describe("apiFetch deadline and Retry-After", () => {
  it("fails with a TimeoutError when the server takes longer than the deadline", async () => {
    server.use(
      http.get(`${ORIGIN}/api/slow`, async () => {
        // Never answers: the deadline or the abort ends the call.
        await delay("infinite");
        return HttpResponse.json({});
      }),
    );
    const err = await apiFetch("/api/slow", { timeout: 20 }).catch((e: unknown) => e);
    expect((err as Error).name).toBe("TimeoutError");
  });

  it("keeps the caller's abort distinct from the deadline", async () => {
    server.use(
      http.get(`${ORIGIN}/api/slow`, async () => {
        await delay("infinite");
        return HttpResponse.json({});
      }),
    );
    const ctrl = new AbortController();
    const p = apiFetch("/api/slow", { signal: ctrl.signal }).catch((e: unknown) => e);
    ctrl.abort();
    expect(((await p) as Error).name).toBe("AbortError");
  });

  it("reads Retry-After and marks 403 as forbidden, not retryable", async () => {
    server.use(
      http.get(
        `${ORIGIN}/api/busy`,
        () => new HttpResponse("slow down", { status: 429, headers: { "Retry-After": "7" } }),
      ),
    );
    const busy = (await apiFetch("/api/busy").catch((e: unknown) => e)) as ProductApiError;
    expect(busy.retryAfterSeconds).toBe(7);
    expect(busy.isRetryable).toBe(true);
    const no = new ProductApiError(403, "requires admin role", "/p", "m");
    expect(no.isForbidden).toBe(true);
    expect(no.isRetryable).toBe(false);
  });
});

describe("withQuery", () => {
  it("drops undefined, null and empty values and keeps the path bare when nothing remains", () => {
    expect(withQuery("/api/x", { a: 1, b: "", c: null, d: undefined, e: false })).toBe(
      "/api/x?a=1&e=false",
    );
    expect(withQuery("/api/x", { a: undefined })).toBe("/api/x");
  });
});
