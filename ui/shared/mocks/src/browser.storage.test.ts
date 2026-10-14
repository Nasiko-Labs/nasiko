// Ported from nasiko-ui-lab@1b99bdda common/src/mocks/browser.storage.test.ts. Local edits: `startMocks()` takes no
// `EnvConfig`; the `nasiko:` storage key; the pinned seed from `@nasiko/testing/pinned-seed`; ESLint autofixes
// (void-returning arrows given braces, redundant casts and `String()` calls dropped); Prettier formatting.
/** Mock mode re-reads its session when another tab clears all storage (ship coverage audit). */
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

describe("storage cleared in another tab (key null)", () => {
  it("re-reads the stored mock sign-out instead of assuming either way", async () => {
    await startMocks();
    expect((await fetch(url("/api/me"))).status).toBe(200);
    localStorage.setItem(KEY, "1");
    window.dispatchEvent(new StorageEvent("storage", { key: null }));
    expect((await fetch(url("/api/me"))).status).toBe(401);
    localStorage.clear();
    window.dispatchEvent(new StorageEvent("storage", { key: null }));
    expect((await fetch(url("/api/me"))).status).toBe(200);
  });

  it("ignores unrelated keys", async () => {
    await startMocks();
    localStorage.setItem(KEY, "1");
    window.dispatchEvent(
      new StorageEvent("storage", { key: "openruntime.theme", newValue: "dark" }),
    );
    expect((await fetch(url("/api/me"))).status).toBe(200);
  });
});
