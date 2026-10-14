// Ported from nasiko-ui-lab@1b99bdda common/src/test/renderApp.tsx, common/src/test/setup.ts (the msw server,
// `recordRequests`, the per-test storage) and common/src/test/pinnedSeed.ts. Local edits: the TanStack router with
// a memory history becomes the navigation-port test harness (`@/test/next-router`) around a small route switch (`/login` renders the
// bare sign-in tree, everything else the `(app)` client providers around a probe page; the server layout's guard
// is not part of a jsdom render, so the principal is passed as the seed admin); the msw server is `@/mocks/node`
// and each test file starts and stops it through `setupShellTests()`; the query client is captured from the tree
// (`useQueryClient`) since the providers own it; Prettier formatting.
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { cleanup, configure, screen } from "@testing-library/react";
import { usePathname, useSearchParams } from "@nasiko/app-core/navigation";
import { useEffect } from "react";
import { afterAll, afterEach, beforeAll, beforeEach, vi } from "vitest";
import * as z from "zod/mini";
import type { Me } from "@nasiko/data-access/product";
import { AppProviders } from "../workspace/app-providers";
import { LoginPage } from "../login/login-page";
import { LoginProviders } from "../login/login-providers";
import { loginSearchSchema } from "../login/search";
import { configureMocks, resetAgentsMock } from "@nasiko/mocks/handlers";
import { server } from "@nasiko/mocks/node";
import { generateSeed } from "@nasiko/mocks/seed";
import { ADMIN_ID } from "@nasiko/mocks/seed-harness";
import { renderWithRouter, routerState } from "@nasiko/testing/router";

export { routerState };

export const FIXED = new Date("2026-03-20T15:00:00Z");
export const seed = generateSeed({ anchor: FIXED });
export const now = () => FIXED.getTime();

/** The seed admin, as the server layout would pass it (`/api/me` answers the same). */
export const ADMIN_ME: Me = { sub: ADMIN_ID, username: "admin", is_superuser: true };
export const SCOPE = { workspaceId: "local", epoch: 0 } as const;

/**
 * A working, per-test localStorage: each test starts with an empty store (the lab's setup; jsdom's own is shared
 * across a file).
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
    removeItem: (k) => {
      m.delete(k);
    },
    setItem: (k, v) => {
      m.set(k, v);
    },
  };
}

/** The msw server, the pinned seed and clock, a fresh storage per test; call once at the top of a test file. */
export function setupShellTests(): void {
  beforeAll(() => {
    server.listen({ onUnhandledFrame: "error" });
    vi.useFakeTimers({ toFake: ["Date"], now: FIXED });
    configureMocks({ seed, now, loggedIn: true, variant: null });
  });
  beforeEach(() => {
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      writable: true,
      value: memoryStorage(),
    });
  });
  afterEach(() => {
    cleanup();
    server.resetHandlers();
    resetAgentsMock();
    configureMocks({ seed, now, loggedIn: true, variant: null });
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });
  afterAll(() => {
    vi.useRealTimers();
    server.close();
  });
}

// jsdom has no layout: the sidebar's nav measures its scroller through ResizeObserver.
class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
// eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- absent in jsdom, whatever lib.dom says
globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;

// Full-page renders on a busy machine can exceed the 1 s findBy default (the lab's 12 s).
configure({ asyncUtilTimeout: 12_000 });

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

const capturedRef: { current: QueryClient | null } = { current: null };

/** A page standing in for a route: its pathname, so tests can see navigations land. */
function Probe() {
  const pathname = usePathname();
  const client = useQueryClient();
  useEffect(() => {
    capturedRef.current = client;
  }, [client]);
  return (
    <section>
      <h1>{pathname}</h1>
    </section>
  );
}

/** The route switch: the bare sign-in page, or the signed-in shell around the probe page. */
function App({ me }: { me: Me }) {
  const pathname = usePathname();
  const params = useSearchParams();
  if (pathname === "/login") {
    const raw: Record<string, string> = {};
    params.forEach((v, k) => {
      raw[k] = v;
    });
    return (
      <LoginProviders>
        <LoginCapture />
        <LoginPage search={z.parse(loginSearchSchema, raw)} />
      </LoginProviders>
    );
  }
  return (
    <AppProviders me={me} scope={SCOPE} initialOpen={undefined}>
      <Probe />
    </AppProviders>
  );
}

function LoginCapture() {
  const client = useQueryClient();
  useEffect(() => {
    capturedRef.current = client;
  }, [client]);
  return null;
}

/** Render the app at `url` (the lab's `renderApp`): the shell or the sign-in page, against the msw handlers. */
export function renderApp(url: string, { me = ADMIN_ME }: { me?: Me } = {}) {
  const result = renderWithRouter(<App me={me} />, { url });
  return {
    ...result,
    /** The query client the tree is using (the shell's, or the sign-in page's). */
    get queryClient(): QueryClient {
      if (!capturedRef.current) throw new Error("no query client rendered yet");
      return capturedRef.current;
    },
  };
}

export const nav = () => screen.getByRole("navigation", { name: "Main" });
export const sidebar = (): HTMLElement => {
  const el = document.querySelector<HTMLElement>('[data-slot="sidebar"]');
  if (!el) throw new Error("no sidebar rendered");
  return el;
};
export const sidebarState = () => sidebar().getAttribute("data-state");
export const wide = () =>
  Object.defineProperty(window, "innerWidth", { configurable: true, value: 1440 });
export const clearCookie = () => {
  document.cookie = "sidebar_state=; path=/; max-age=0";
};
