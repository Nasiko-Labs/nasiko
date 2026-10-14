/** The product QueryClient: retry policy, Retry-After, mutations never retry, the one 401 handler, onCacheReset. */
import { MutationObserver } from "@tanstack/query-core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ME_FEATURE } from "./auth";
import { ProductApiError } from "./client";
import { productKey } from "./keys";
import { createProductQueryClient, onCacheReset } from "./query-client";

const scope = { workspaceId: "ws-a", epoch: 0 };
const fail = (status: number, retryAfter: number | null = null) =>
  new ProductApiError(status, null, "/api/x", `GET /api/x → ${status}`, retryAfter);

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

/** Runs the query to its settled state, advancing fake timers through the retry delays. */
async function settle(p: Promise<unknown>): Promise<unknown> {
  const out = p.catch((e: unknown) => e);
  for (let i = 0; i < 10; i++) {
    await vi.advanceTimersByTimeAsync(31_000);
  }
  return out;
}

describe("retry policy", () => {
  it("a 500 retries twice (three attempts) and then surfaces the error", async () => {
    const qc = createProductQueryClient({ onUnauthorized: () => undefined });
    const queryFn = vi.fn().mockRejectedValue(fail(500));
    const res = await settle(qc.query({ queryKey: productKey(scope, "f"), queryFn }));
    expect(queryFn).toHaveBeenCalledTimes(3);
    expect((res as ProductApiError).status).toBe(500);
  });

  it("a 404 (or 403, or a schema failure) never retries", async () => {
    const qc = createProductQueryClient({ onUnauthorized: () => undefined });
    for (const status of [404, 403, 200]) {
      const queryFn = vi.fn().mockRejectedValue(fail(status));
      await settle(qc.query({ queryKey: productKey(scope, "f", status), queryFn }));
      expect(queryFn).toHaveBeenCalledTimes(1);
    }
  });

  it("a deadline (TimeoutError) retries; a caller's abort does not", async () => {
    const qc = createProductQueryClient({ onUnauthorized: () => undefined });
    const timeout = vi.fn().mockRejectedValue(new DOMException("deadline", "TimeoutError"));
    await settle(qc.query({ queryKey: productKey(scope, "t"), queryFn: timeout }));
    expect(timeout).toHaveBeenCalledTimes(3);
    const abort = vi.fn().mockRejectedValue(new DOMException("gone", "AbortError"));
    await settle(qc.query({ queryKey: productKey(scope, "a"), queryFn: abort }));
    expect(abort).toHaveBeenCalledTimes(1);
  });

  it("honours Retry-After (capped at 30 s) before the next attempt", async () => {
    const qc = createProductQueryClient({ onUnauthorized: () => undefined });
    const queryFn = vi.fn().mockRejectedValueOnce(fail(429, 7)).mockResolvedValue("ok");
    const p = qc.query({ queryKey: productKey(scope, "busy"), queryFn });
    await vi.advanceTimersByTimeAsync(6_900);
    expect(queryFn).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(200);
    expect(queryFn).toHaveBeenCalledTimes(2);
    expect(await p).toBe("ok");
  });

  it("retry: false turns retries off", async () => {
    const qc = createProductQueryClient({ onUnauthorized: () => undefined, retry: false });
    const queryFn = vi.fn().mockRejectedValue(fail(503));
    await settle(qc.query({ queryKey: productKey(scope, "f"), queryFn }));
    expect(queryFn).toHaveBeenCalledTimes(1);
  });

  it("mutations never retry, even on a 500", async () => {
    const qc = createProductQueryClient({ onUnauthorized: () => undefined });
    const mutationFn = vi.fn().mockRejectedValue(fail(500));
    const m = new MutationObserver(qc, { mutationFn });
    const res = await settle(m.mutate(undefined));
    expect(mutationFn).toHaveBeenCalledTimes(1);
    expect((res as ProductApiError).status).toBe(500);
  });
});

describe("the 401 handler", () => {
  it("a 401 on a query clears the cache and calls onUnauthorized once per burst", async () => {
    const onUnauthorized = vi.fn();
    const qc = createProductQueryClient({ onUnauthorized });
    qc.setQueryData(productKey(scope, "agents"), [1, 2]);
    const results = await Promise.all([
      settle(
        qc.query({
          queryKey: productKey(scope, "a"),
          queryFn: () => Promise.reject(fail(401)),
        }),
      ),
      settle(
        qc.query({
          queryKey: productKey(scope, "b"),
          queryFn: () => Promise.reject(fail(401)),
        }),
      ),
    ]);
    expect(results.every((r) => r instanceof ProductApiError)).toBe(true);
    expect(onUnauthorized).toHaveBeenCalledTimes(1);
    expect(qc.getQueryData(productKey(scope, "agents"))).toBeUndefined();
    // Once the reaction settled, a later expiry reacts again.
    await settle(
      qc.query({ queryKey: productKey(scope, "c"), queryFn: () => Promise.reject(fail(401)) }),
    );
    expect(onUnauthorized).toHaveBeenCalledTimes(2);
  });

  it("a 401 on a mutation takes the same path", async () => {
    const onUnauthorized = vi.fn();
    const qc = createProductQueryClient({ onUnauthorized });
    const m = new MutationObserver(qc, { mutationFn: () => Promise.reject(fail(401)) });
    await settle(m.mutate(undefined));
    expect(onUnauthorized).toHaveBeenCalledTimes(1);
  });

  it("a first-load `me` 401 belongs to the route guard; a later one is an expiry", async () => {
    const onUnauthorized = vi.fn();
    const qc = createProductQueryClient({ onUnauthorized });
    const me = productKey(scope, ME_FEATURE);
    await settle(
      qc.query({ queryKey: me, queryFn: () => Promise.reject(fail(401)), retry: false }),
    );
    expect(onUnauthorized).not.toHaveBeenCalled();
    qc.setQueryData(me, { sub: "u1" });
    await settle(
      qc.query({
        queryKey: me,
        queryFn: () => Promise.reject(fail(401)),
        retry: false,
        staleTime: 0,
      }),
    );
    expect(onUnauthorized).toHaveBeenCalledTimes(1);
  });

  it("other errors never reach onUnauthorized", async () => {
    const onUnauthorized = vi.fn();
    const qc = createProductQueryClient({ onUnauthorized });
    await settle(
      qc.query({ queryKey: productKey(scope, "x"), queryFn: () => Promise.reject(fail(403)) }),
    );
    await settle(
      qc.query({
        queryKey: productKey(scope, "y"),
        queryFn: () => Promise.reject(new Error("boom")),
      }),
    );
    expect(onUnauthorized).not.toHaveBeenCalled();
  });
});

describe("onCacheReset", () => {
  it("runs on clear(), on an unfiltered resetQueries() and when a new client is built; unsubscribes", async () => {
    const fn = vi.fn();
    const off = onCacheReset(fn);
    const qc = createProductQueryClient({ onUnauthorized: () => undefined });
    expect(fn).toHaveBeenCalledTimes(1); // a new cache is a new session
    qc.clear();
    expect(fn).toHaveBeenCalledTimes(2);
    await qc.resetQueries({ queryKey: productKey(scope, "f") });
    expect(fn).toHaveBeenCalledTimes(2); // filtered: not a session reset
    await qc.resetQueries();
    expect(fn).toHaveBeenCalledTimes(3);
    off();
    qc.clear();
    expect(fn).toHaveBeenCalledTimes(3);
  });
});
