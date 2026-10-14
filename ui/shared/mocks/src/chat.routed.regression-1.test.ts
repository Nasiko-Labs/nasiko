// Ported from nasiko-ui-lab@1b99bdda common/src/mocks/chat.routed.regression-1.test.ts. Local edits: the file starts
// the msw/node server itself (the lab's global setup did); ESLint autofixes (void-returning arrows given braces,
// redundant casts and `String()` calls dropped); Prettier formatting. Regression: ISSUE-004 — a routed turn's flow
// (chat's mock) failed the Flows page's schema: no created_at, no step ids, no title, so chat → Open flow showed
// "Couldn't load this flow". Found by /qa on 2026-10-06 Report: .gstack/qa-reports/qa-report-localhost-2026-10-06.md
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { flowDetailSchema } from "@nasiko/features/flows/types";
import { readSse } from "@nasiko/data-access/product";
import { configureChatMock, resetChatMock } from "./chatStore";
import { configureMocks, resetAgentsMock } from "./handlers";
import { server } from "./node";

// The lab's global test setup started msw for every test file; here a file that fetches without
// `setupPinnedSeed()` starts the msw/node server itself.
beforeAll(() => {
  server.listen({ onUnhandledFrame: "error" });
});
afterEach(() => {
  server.resetHandlers();
  resetAgentsMock();
});
afterAll(() => {
  server.close();
});

const url = (p: string) => new URL(p, globalThis.location.origin);
const post = (p: string, body: unknown) =>
  fetch(url(p), {
    method: "POST",
    body: JSON.stringify(body),
    headers: { "Content-Type": "application/json" },
  });

beforeEach(() => {
  configureMocks({ loggedIn: true });
});
afterEach(() => {
  resetChatMock();
});

describe("a routed turn’s flow (ISSUE-004)", () => {
  it("has the server’s shape: the Flows page can open it, titled with the user’s message", async () => {
    configureChatMock({ scenario: "routed-multi-agent" });
    const sid = "5eedc000-0000-4000-8000-00000000qa04";
    const text = "compare the two vendors";
    await post("/api/chat/sessions", { session_id: sid, first_prompt: text });
    const res = await post("/api/orchestrator/a2a", {
      jsonrpc: "2.0",
      id: 1,
      method: "message/stream",
      params: {
        message: { messageId: "m", role: "ROLE_USER", parts: [{ text }], contextId: sid },
        metadata: { session_id: sid },
      },
    });
    const frames: string[] = [];
    await readSse(res, (evs) => {
      evs.forEach((e) => frames.push(e.data));
    });
    const trace = frames.join("").match(/"trace_id":"(5eedf\w+)"/)?.[1] ?? "";
    const body: unknown = await (await fetch(url(`/api/flows/${trace}`))).json();
    const parsed = flowDetailSchema.safeParse(body);
    expect(parsed.success).toBe(true);
    if (!parsed.success) return;
    expect(parsed.data.flow).toMatchObject({
      title: text,
      root_agent_name: "orchestrator",
      metadata: { context_id: sid },
    });
    expect(parsed.data.steps.length).toBeGreaterThan(0);
    expect(new Set(parsed.data.steps.map((s) => s.id)).size).toBe(parsed.data.steps.length);
  });
});
