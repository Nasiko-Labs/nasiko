// Ported from nasiko-ui-lab@1b99bdda common/src/test/pinnedSeed.ts. Local edits: `setupPinnedSeed()` also starts and
// stops the msw/node `server` (shared/mocks/src/node.ts) and resets its handlers after each test, the part of the
// lab's setup.ts that was global there; `recordRequests` and `recordRequestBodies` move here from the lab's setup.ts; the harness seed is
// pinned to the same clock (the lab's handlers default to it; here the finops aggregations read it explicitly);
// `setupMockServer()` is the same server lifecycle without the pinned seed (the lab's global one, for files that pin
// none); Prettier formatting.
/**
 * Shared page-test setup: one fixed clock and the seed generated for it, so results never
 * depend on the run date. Call `setupPinnedSeed()` at the top of a test file.
 */
import { screen } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, vi } from "vitest";
import { configureMocks, resetAgentsMock } from "@nasiko/mocks/handlers";
import { server } from "@nasiko/mocks/node";
import { generateSeed } from "@nasiko/mocks/seed";
import { generateHarnessSeed } from "@nasiko/mocks/seed-harness";

export { server };

export const FIXED = new Date("2026-03-20T15:00:00Z");
export const seed = generateSeed({ anchor: FIXED });
export const harnessSeed = generateHarnessSeed({ anchor: FIXED });
export const now = () => FIXED.getTime();

/** Pin Date to FIXED and the mocks to `seed`; after each test, reset mocks, stubbed globals and spies. */
export function setupPinnedSeed(): void {
  beforeAll(() => {
    vi.useFakeTimers({ toFake: ["Date"], now: FIXED });
    configureMocks({ seed, now, harnessSeed, loggedIn: true, variant: null });
    server.listen({ onUnhandledFrame: "error" });
  });
  afterAll(() => {
    vi.useRealTimers();
    server.close();
  });
  afterEach(() => {
    server.resetHandlers();
    resetAgentsMock();
    configureMocks({ seed, now, harnessSeed, loggedIn: true, variant: null });
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });
}

/**
 * The lab's global msw lifecycle (common/src/test/setup.ts: listen before the file, reset the handlers and the agents
 * mock after each test, close after) for a test file that pins no seed: the lab's chat tests run on the real clock
 * against the default mocks. Use `setupPinnedSeed()` instead when the file pins the seed.
 */
export function setupMockServer(): void {
  beforeAll(() => {
    server.listen({ onUnhandledFrame: "error" });
  });
  afterAll(() => {
    server.close();
  });
  afterEach(() => {
    server.resetHandlers();
    resetAgentsMock();
  });
}

/**
 * A disclosure's button (`#disclosure-<id>-title`). By id rather than by name: Base UI's time-control toggles are
 * buttons too (the lab's Radix ones were radios), so a name like /^This month/ now matches both.
 */
export const disclosure = (id: string): HTMLElement =>
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion -- every disclosure renders its titled button
  document.getElementById(`disclosure-${id}-title`)!;

/** The <section> a panel heading belongs to. */
export const section = async (heading: RegExp | string) =>
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion -- every panel heading sits in its section
  (await screen.findByRole("heading", { name: heading })).closest("section")!;

/** Record the URL of every request the page makes; call `stop()` in a finally block. */
export function recordRequests() {
  const urls: URL[] = [];
  const onStart = ({ request }: { request: Request }) => {
    urls.push(new URL(request.url));
  };
  server.events.on("request:start", onStart);
  return {
    urls,
    stop: () => {
      server.events.removeListener("request:start", onStart);
    },
  };
}

// The lab's `recordRequestBodies` (common/src/test/setup.ts), against this package's `server`.
export interface RecordedBody {
  method: string;
  url: URL;
  /** Parsed JSON, the raw text when it isn't JSON, or null with no body. */
  body: unknown;
}

/**
 * Record every request's method, URL and parsed body (v1b EN-11). Each request is cloned before
 * reading, so handlers still see the original body. Assert only after `await flush()`, which
 * waits for pending body reads; `parseErrors` counts bodies that couldn't be read at all.
 */
export function recordRequestBodies() {
  const requests: RecordedBody[] = [];
  const pending = new Set<Promise<void>>();
  let parseErrors = 0;
  const onStart = ({ request }: { request: Request }) => {
    const clone = request.clone();
    const p = clone
      .text()
      .then(
        (raw) => {
          let body: unknown = raw || null;
          try {
            body = raw ? JSON.parse(raw) : null;
          } catch {
            /* not JSON: keep the text */
          }
          requests.push({ method: request.method, url: new URL(request.url), body });
        },
        () => {
          parseErrors++;
        },
      )
      .finally(() => pending.delete(p));
    pending.add(p);
  };
  server.events.on("request:start", onStart);
  return {
    requests,
    get parseErrors() {
      return parseErrors;
    },
    async flush() {
      while (pending.size) await Promise.all([...pending]);
    },
    /**
     * POSTs that would save an assistant row: the client must never send one on a routed chat.
     * Fails closed: an unreadable body could have been one.
     */
    assistantPosts: () => {
      if (parseErrors) throw new Error(`${parseErrors} request bodies couldn't be read`);
      return requests.filter(
        (r) =>
          r.method === "POST" &&
          /\/api\/chat\/sessions\/[^/]+\/messages$/.test(r.url.pathname) &&
          (r.body as { role?: unknown } | null)?.role === "assistant",
      );
    },
    stop: () => {
      server.events.removeListener("request:start", onStart);
    },
  };
}
