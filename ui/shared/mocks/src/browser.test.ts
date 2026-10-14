// Ported from nasiko-ui-lab@1b99bdda common/src/mocks/browser.test.ts. Local edits: `startMocks()` takes no
// `EnvConfig` (no partial-live mode here), so the live-mode and partial-live cases are replaced by an edition-stack
// and pinned-session case and an onboarding-persistence case; msw 3's `onUnhandledFrame`; `nasiko:` storage keys; the
// pinned seed from `@nasiko/testing/pinned-seed`; ESLint autofixes (void-returning arrows given braces, redundant
// casts and `String()` calls dropped); Prettier formatting.
/* eslint-disable @typescript-eslint/no-non-null-assertion -- the seed's shape is what the test asserts */
/**
 * The browser mock bootstrap (QA ISSUE-003): a mock sign-out survives a reload through
 * localStorage, blocked storage starts signed in, the edition's handlers answer first and the session is the seed admin.
 * The service worker itself is stubbed; the handlers are the same ones msw/node serves in tests.
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { setupPinnedSeed } from "@nasiko/testing/pinned-seed";
import { http, HttpResponse } from "msw";
import { allHandlers, configureMocks } from "./handlers";
import { ADMIN_ID } from "./seed-harness";
import { startMocks } from "./browser";

const start = vi.fn(async () => {});
const setupWorker = vi.fn((..._handlers: unknown[]) => ({ start }));
vi.mock("msw/browser", () => ({ setupWorker: (...h: unknown[]) => setupWorker(...h) }));

setupPinnedSeed();

const KEY = "nasiko:mock-signed-out";
const url = (p: string) => new URL(p, globalThis.location.origin).toString();

function memoryStorage(): Storage {
  const m = new Map<string, string>();
  return {
    get length() {
      return m.size;
    },
    clear: () => {
      m.clear();
    },
    getItem: (k) => m.get(k) ?? null,
    key: (i) => [...m.keys()][i] ?? null,
    removeItem: (k) => void m.delete(k),
    setItem: (k, v) => void m.set(k, v),
  };
}

afterEach(() => {
  configureMocks({
    persistLogin: null,
    persona: null,
    lockPersona: false,
    session: null,
    persistOnboarding: null,
    onboarding: { persona: "developer", completed: true },
  });
  setupWorker.mockClear();
  start.mockClear();
});

describe("startMocks", () => {
  it("starts signed out after a stored mock sign-out, and records the next sign-in", async () => {
    const store = memoryStorage();
    store.setItem(KEY, "1");
    vi.stubGlobal("localStorage", store);
    await startMocks();
    expect(setupWorker).toHaveBeenCalledTimes(1);
    expect(start).toHaveBeenCalledWith(expect.objectContaining({ onUnhandledFrame: "bypass" }));
    expect((await fetch(url("/api/me"))).status).toBe(401);
    await fetch(url("/api/auth/login"), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: "admin", password: "x" }),
    });
    expect(store.getItem(KEY)).toBeNull();
    await fetch(url("/api/auth/logout"), { method: "POST" });
    expect(store.getItem(KEY)).toBe("1");
  });

  it("starts signed in when storage is blocked, and a sign-out never throws", async () => {
    const fail = () => {
      throw new Error("blocked");
    };
    vi.stubGlobal("localStorage", {
      length: 0,
      clear: fail,
      getItem: fail,
      key: fail,
      removeItem: fail,
      setItem: fail,
    } satisfies Storage);
    await startMocks();
    expect((await fetch(url("/api/me"))).status).toBe(200);
    expect((await fetch(url("/api/auth/logout"), { method: "POST" })).status).toBe(204);
  });

  it("stacks the edition's handlers in front of the core's, and pins the session to the seed admin", async () => {
    const edition = http.get("/api/me", () => HttpResponse.json({ edition: true }));
    await startMocks({ editionHandlers: { auth: [edition] } });
    expect(setupWorker).toHaveBeenCalledTimes(1);
    const handlers = setupWorker.mock.calls[0]!;
    expect(handlers[0]).toBe(edition);
    expect(handlers).toHaveLength(allHandlers.length + 1);
    // A persona picks the harness viewer, never the session: the server's mock principal is the seed admin.
    configureMocks({ persona: "maya" });
    const me = (await (await fetch(url("/api/me"))).json()) as { sub: string };
    expect(me.sub).toBe(ADMIN_ID);
  });

  it("starts as a first-time user and keeps a picked persona across reloads", async () => {
    const store = memoryStorage();
    vi.stubGlobal("localStorage", store);
    await startMocks();
    const read = async () => (await fetch(url("/api/me/onboarding"))).json() as Promise<unknown>;
    expect(await read()).toEqual({ is_first_time_user: true, persona: null });
    await fetch(url("/api/me/onboarding"), {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ persona: "finance" }),
    });
    expect(JSON.parse(store.getItem("nasiko:mock-onboarding") ?? "null")).toEqual({
      persona: "finance",
      completed: true,
    });
  });
});
