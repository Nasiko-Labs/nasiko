// Ported from nasiko-ui-lab@1b99bdda common/src/mocks/browser.tabs.test.ts. Local edits: `startMocks()` takes no
// `EnvConfig`; the `nasiko:` storage key; the pinned seed from `@nasiko/testing/pinned-seed`; ESLint autofixes
// (void-returning arrows given braces, redundant casts and `String()` calls dropped); Prettier formatting.
/** Mock mode follows sign-ins and sign-outs made in other tabs, as one shared cookie would (ship review). */
import { afterEach, describe, expect, it, vi } from "vitest";
import { setupPinnedSeed } from "@nasiko/testing/pinned-seed";
import { configureMocks } from "./handlers";
import { startMocks } from "./browser";

vi.mock("msw/browser", () => ({ setupWorker: () => ({ start: async () => {} }) }));
setupPinnedSeed();

const KEY = "nasiko:mock-signed-out";
const url = (p: string) => new URL(p, globalThis.location.origin).toString();
afterEach(() => {
  configureMocks({ persistLogin: null, loggedIn: true });
});

describe("mock session across tabs", () => {
  it("another tab signing out ends this tab’s mock session, and signing in restores it", async () => {
    const m = new Map<string, string>();
    vi.stubGlobal("localStorage", {
      get length() {
        return m.size;
      },
      clear: () => {
        m.clear();
      },
      getItem: (k: string) => m.get(k) ?? null,
      key: (i: number) => [...m.keys()][i] ?? null,
      removeItem: (k: string) => void m.delete(k),
      setItem: (k: string, v: string) => void m.set(k, v),
    } satisfies Storage);
    await startMocks();
    expect((await fetch(url("/api/me"))).status).toBe(200);
    window.dispatchEvent(new StorageEvent("storage", { key: KEY, newValue: "1" }));
    expect((await fetch(url("/api/me"))).status).toBe(401);
    window.dispatchEvent(new StorageEvent("storage", { key: KEY, newValue: null }));
    expect((await fetch(url("/api/me"))).status).toBe(200);
  });
});
