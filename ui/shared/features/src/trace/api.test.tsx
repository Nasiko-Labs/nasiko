// Ported from nasiko-ui-lab@1b99bdda common/src/features/trace/api.test.tsx. Local edits: the lab's
// `createQueryClient(getRouter, { retry: false })` with a stub is `createProductQueryClient({ onUnauthorized, retry:
// false })`, provided with a scope through `ProductQueryProvider` (the hooks key by it); `setupMockServer()` (the lab's global msw lifecycle, which this workspace's setup does not run); Prettier formatting.
import { renderHook, waitFor } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useSessionStatuses } from "../sessions/api";
import type { FlatSpan } from "../observability/spans";
import { ARRIVAL_POLL_MS, ARRIVAL_POLLS } from "../observability/tuning";
import type { SessionSummary } from "../observability/types";
import { createProductQueryClient, ProductQueryProvider } from "@nasiko/data-access/product";
import { server, setupMockServer } from "@nasiko/testing/pinned-seed";
import { useAgentCallTargets, useTraceDetail, useWasteCosts } from "./api";

const OBS = "/api/observability";

// The lab's global test setup started the msw server for every file; here the file asks for it.
setupMockServer();

const SCOPE = { workspaceId: "local", epoch: 0 } as const;

function wrapper() {
  const client = createProductQueryClient({ onUnauthorized: () => undefined, retry: false });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <ProductQueryProvider scope={SCOPE} queryClient={client}>
        {children}
      </ProductQueryProvider>
    );
  };
}

afterEach(() => vi.useRealTimers());

describe("useTraceDetail arrival budget", () => {
  it("counts failed re-polls: one fetch plus ARRIVAL_POLLS re-polls, then it stops", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "setTimeout", "clearTimeout"] });
    let calls = 0;
    server.use(
      http.get(`${OBS}/trace/:id`, () => {
        calls++;
        return calls === 1
          ? HttpResponse.json({ data: { trace: { spans: [], span_lookup: {}, num_spans: 1 } } })
          : new HttpResponse("trace store unavailable", { status: 502 });
      }),
    );
    const { result } = renderHook(() => useTraceDetail("t-arrival", true), { wrapper: wrapper() });
    await vi.waitFor(() => {
      expect(result.current.data).toBeDefined();
    });
    await vi.advanceTimersByTimeAsync(ARRIVAL_POLL_MS * (ARRIVAL_POLLS + 4));
    expect(calls).toBe(1 + ARRIVAL_POLLS);
  });
});

describe("stable results", () => {
  const span = (id: string) => ({ node: { id, span_id: `hex${id}` } }) as unknown as FlatSpan;

  it("span fan-outs return the same Map across renders once answered", async () => {
    server.use(
      http.get(`${OBS}/span/:trace/:span`, () =>
        HttpResponse.json({
          data: {
            span: {
              attributes: { agent: { id: "callee" } },
              cost_summary: { total: { cost: 0.5 } },
            },
          },
        }),
      ),
    );
    const spans = [span("a"), span("b")];
    const { result, rerender } = renderHook(
      () => ({ costs: useWasteCosts("t", spans, true), targets: useAgentCallTargets("t", spans) }),
      { wrapper: wrapper() },
    );
    await waitFor(() => {
      expect(result.current.costs?.get("a")).toBe(0.5);
    });
    await waitFor(() => {
      expect(result.current.targets.get("b")).toBe("callee");
    });
    const before = result.current;
    rerender();
    expect(result.current.costs).toBe(before.costs);
    expect(result.current.targets).toBe(before.targets);
  });

  it("useSessionStatuses returns the same Map across renders once settled", async () => {
    server.use(
      http.get(`${OBS}/session/:id`, () =>
        HttpResponse.json({
          data: {
            session: {
              traces: [
                { trace_id: "t-ok", root_span: { cumulative_token_count_total: 1, trace: {} } },
              ],
            },
          },
        }),
      ),
      http.get(`${OBS}/trace/:id`, () =>
        HttpResponse.json({ data: { trace: { spans: [], span_lookup: {}, num_spans: 0 } } }),
      ),
    );
    const rows = [
      { session_id: "s1", end_time: "2020-01-01T00:00:00Z" },
    ] as unknown as SessionSummary[];
    const now = Date.parse("2020-01-02T00:00:00Z");
    const { result, rerender } = renderHook(() => useSessionStatuses(rows, now, false), {
      wrapper: wrapper(),
    });
    await waitFor(() => {
      expect(result.current.get("s1")).toBe("ok");
    });
    const before = result.current;
    rerender();
    expect(result.current).toBe(before);
  });
});
