// Ported from nasiko-ui-lab@1b99bdda common/src/test/setup.ts (the jsdom shims and the Testing Library timeout).
// Local edits: animations finish instantly through `skipAnimationsInTests()` (the lab's
// `MotionGlobalConfig.skipAnimations = true`, restored for the full feature port); the msw/node server is started per test file by `setupPinnedSeed()` (./pinned-seed.ts) rather than
// here, so the mocks' own tests (oss/app/src/mocks, which start the same `server`) are unaffected; the chat registry reset (the lab's DX3)
// runs after each test as in the lab (the cleanup and mock resets it sat beside live in dom-setup.ts and pinned-seed.ts);
// the edition's own handlers are stacked by the Enterprise test setup; an empty `getAnimations` shim (jsdom gap, Base
// UI's ScrollArea); Prettier formatting.
import { configure } from "@testing-library/react";
import { skipAnimationsInTests } from "@nasiko/ui/motion/testing";
import { afterEach, beforeEach } from "vitest";

/**
 * A working, per-test localStorage. Node 25's built-in global shadows jsdom's and throws on use
 * (it needs --localstorage-file), which silently turned every storage read into "unavailable" and
 * let state leak between tests. Each test starts with an empty store.
 */
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
// jsdom has no pointer-capture API; the Select primitive calls it when it opens. Node-environment test
// files (`@vitest-environment node`) have no Element.
if (typeof Element !== "undefined") {
  const proto = Element.prototype as unknown as Record<string, unknown>;
  for (const k of ["hasPointerCapture", "releasePointerCapture", "setPointerCapture"]) {
    if (!(k in proto)) proto[k] = k === "hasPointerCapture" ? () => false : () => {};
  }
  // jsdom has no Web Animations API: Base UI's ScrollArea viewport asks for its subtree's animations on a timer after
  // mount, which threw as an unhandled error in any test whose page has a scroll area (the chat rail, deploy's lists).
  if (!("getAnimations" in proto)) proto.getAnimations = () => [];
}

beforeEach(() => {
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    writable: true,
    value: memoryStorage(),
  });
  // jsdom's own sessionStorage works (tests spy on `Storage.prototype`, so it is kept), but it lives for the whole
  // file: empty it, so /login's external sign-in mark (session.ts markExternalSignIn) never leaks between tests.
  try {
    globalThis.sessionStorage.clear();
  } catch {
    // No sessionStorage in this environment.
  }
});

// jsdom has no layout: visx's ParentSize and the primitives need ResizeObserver to exist.
class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
// eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- jsdom's globals lack it
globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;
// jsdom has no scrollIntoView; the page's scroll-to-section calls are optional, list items' are not. Nor scrollTo
// (the assistant thread's viewport scrolls itself; development 66a4c64d0).
if (typeof Element !== "undefined") {
  // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition, @typescript-eslint/unbound-method -- jsdom's Element lacks it
  Element.prototype.scrollIntoView ??= function scrollIntoView() {};
  // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition, @typescript-eslint/unbound-method -- jsdom's Element lacks it
  Element.prototype.scrollTo ??= function scrollTo() {};
}

// Full-page renders under a parallel run (or coverage instrumentation) can exceed the 1 s
// findBy default on a busy machine, and a file's first render also pays for the React Compiler's
// Babel pass over the page's modules; 12 s keeps page tests deterministic without masking hangs.
configure({ asyncUtilTimeout: 12_000 });

// Animations finish instantly in tests: assertions never wait on a spring (the lab's setup).
skipAnimationsInTests();

// Chat's per-user state (turns, signals, the request index, the poll backoff) never leaks between tests (DX3).
// Imported lazily, so a test file's vi.mock of a chat module still applies to it (and one that fails to load is skipped).
afterEach(async () => {
  await import("@nasiko/features/chat/registry").then(
    (m) => {
      m.clearChatRegistry();
    },
    () => undefined,
  );
});
