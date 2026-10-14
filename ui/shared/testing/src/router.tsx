// The navigation-port harness for jsdom tests (no lab counterpart: the lab rendered its TanStack router with a memory
// history, `common/src/test/renderApp.tsx`). Written during the Next port and kept for the SPA's navigation port: it
// supplies the port (`NavigationContext`) and its leave blocker (`LeaveBlockerContext`, the same rules as the app's
// TanStack blocker, router-provider.tsx) over an in-memory history. Local edits: n/a (new file).
/**
 * Renders a tree under the navigation port with an in-memory history, so `useRouter`, `usePathname`,
 * `useSearchParams` and the port's `Link` work as they do in the browser:
 *
 *   const { router } = renderWithRouter(<Page />, { url: "/agents?preset=7d" });
 *   await userEvent.click(screen.getByRole("link", { name: "TokenOps" }));
 *   expect(routerState().pathname).toBe("/tokenops");
 *
 * - `push` / `replace` / `back` / `forward` move an entry index over a URL list (`routerState().index`),
 *   re-rendering the contexts; an active leave guard holds them (`ignoreBlocker` passes); `prefetch` and `refresh`
 *   are no-ops.
 * - `routerState()` reads the current entry: `{ pathname, search, href, index, length }`. `search` is a
 *   `URLSearchParams`; `href` is pathname plus the query, like the lab's `router.state.location.href`.
 * - `navigate(url)` moves the harness from outside React (a test standing in for the browser's address bar).
 * - A plain click on a same-origin `<a href>` that no handler took (the port's `Link` navigates through `push`
 *   itself) is turned into `push(href)`, so clicking links navigates in jsdom as it does in the browser. Modified
 *   clicks, `target`ed links and clicks a handler already cancelled (`preventDefault`) are left alone.
 * - `window.history` is the harness's too: `pushState` / `replaceState` (what `router.push`/`replace` call here) add
 *   or replace a harness entry and keep its `state`, numbered `__TSR_index` as the app's router numbers its entries
 *   (url-state.ts `historyIndex()` reads it); `history.state` reads the current entry's; `back`/`forward` move the
 *   index and fire `popstate` with that entry's state.
 * - The fragment reaches the page through the port (`hash`, url-state.ts `useHash`); jsdom's `location.hash` follows
 *   the current entry's too. jsdom's path is left alone: the port is the only thing a page should read.
 */
import { render, type RenderOptions, type RenderResult } from "@testing-library/react";
import {
  LeaveBlockerContext,
  NavigationContext,
  type LeaveBlocker,
  type LeaveBlockerOptions,
  type Navigation,
} from "@nasiko/app-core/navigation";
import {
  useEffect,
  useState,
  useSyncExternalStore,
  type ReactElement,
  type ReactNode,
} from "react";

export interface RouterLocation {
  readonly pathname: string;
  readonly search: URLSearchParams;
  /** pathname + query (+ hash), the form a redirect carries. */
  readonly href: string;
  /** The entry's position in the history list (what `history.state.index` reads). */
  readonly index: number;
  readonly length: number;
}

interface MemoryHistory {
  entries: string[];
  /** Each entry's `history.state` (what `pushState`/`replaceState` were given). */
  states: unknown[];
  index: number;
}

const history: MemoryHistory = { entries: ["/"], states: [null], index: 0 };
const listeners = new Set<() => void>();
/** One snapshot per history change: `useSyncExternalStore` needs a stable object between changes. */
let snapshot: RouterLocation | null = null;

function emit(): void {
  snapshot = null;
  syncHash();
  for (const fn of listeners) fn();
}

/**
 * jsdom's fragment follows the current entry's (the page's `useHash` reads `location.hash`, as the browser's
 * pushState updates it). Only the fragment: jsdom's path stays where it was (see the header).
 */
function syncHash(): void {
  if (typeof window === "undefined") return;
  const { hash } = parse(history.entries[history.index] ?? "/");
  if (window.location.hash !== hash) window.location.hash = hash;
}

function parse(href: string): { pathname: string; search: string; hash: string } {
  const u = new URL(href, "http://harness.test");
  return { pathname: u.pathname, search: u.search, hash: u.hash };
}

/** The current entry. Also readable outside React (assertions). */
export function routerState(): RouterLocation {
  if (snapshot) return snapshot;
  const href = history.entries[history.index] ?? "/";
  const { pathname, search } = parse(href);
  snapshot = {
    pathname,
    search: new URLSearchParams(search),
    href,
    index: history.index,
    length: history.entries.length,
  };
  return snapshot;
}

/** Resolve `href` the way the browser would from the current entry (relative paths, `?query` only, etc.). */
function resolve(href: string): string {
  const base = new URL(routerState().href, "http://harness.test");
  const u = new URL(href, base);
  return u.pathname + u.search + u.hash;
}

/** Number an entry as the app's router does (`__TSR_index` in its state; url-state.ts `historyIndex()` reads it). */
function stamp(state: unknown, index: number): object {
  return { ...(state && typeof state === "object" ? state : {}), __TSR_index: index };
}

function push(href: string, state: unknown = null): void {
  const index = history.index + 1;
  history.entries = [...history.entries.slice(0, index), resolve(href)];
  history.states = [...history.states.slice(0, index), stamp(state, index)];
  history.index = index;
  emit();
}

function replace(href: string, state: unknown = null): void {
  history.entries = history.entries.map((e, i) => (i === history.index ? resolve(href) : e));
  history.states = history.states.map((s, i) => (i === history.index ? stamp(state, i) : s));
  emit();
}

function popTo(index: number): void {
  history.index = index;
  emit();
  window.dispatchEvent(new PopStateEvent("popstate", { state: history.states[index] ?? null }));
}

function back(): void {
  if (history.index === 0) return;
  popTo(history.index - 1);
}

function forward(): void {
  if (history.index >= history.entries.length - 1) return;
  popTo(history.index + 1);
}

let historyInstalled = false;
/**
 * Route jsdom's `window.history` through the harness (once per test file; jsdom gives each file its own window).
 * Installed before the first render, so `url-state.ts`'s index stamp wraps these functions as it wraps the
 * browser's.
 */
function installHistory(): void {
  if (historyInstalled || typeof window === "undefined") return;
  historyInstalled = true;
  const h = window.history;
  h.pushState = (data: unknown, _unused: string, url?: string | URL | null) => {
    push(url == null ? routerState().href : String(url), data);
  };
  h.replaceState = (data: unknown, _unused: string, url?: string | URL | null) => {
    replace(url == null ? routerState().href : String(url), data);
  };
  h.back = back;
  h.forward = forward;
  Object.defineProperty(h, "state", {
    configurable: true,
    get: () => history.states[history.index] ?? null,
  });
}

/** Move the harness from outside React, as the address bar would (a full navigation: a new entry). */
export function navigate(href: string): void {
  installHistory();
  window.history.pushState(null, "", href);
}

/** Start the history over at `url` (renderWithRouter does this; a test may call it between renders). */
export function resetRouter(url = "/"): void {
  installHistory();
  history.entries = [url.startsWith("/") ? url : `/${url}`];
  history.states = [stamp(null, 0)];
  history.index = 0;
  emit();
}

/** An active leave guard's blocker (the harness's side of the navigation port's `LeaveBlockerContext`). */
interface HarnessBlocker {
  /** Should a move to `target` (resolved href) be held? */
  shouldBlock: (target: string) => boolean;
  /** Hold `go` until the guard's proceed (which runs it) or reset (which drops it). */
  hold: (go: () => void) => void;
}
const blockers = new Set<HarnessBlocker>();

/** Run `go` (a move to `target`), unless an active guard holds it, as the app's router blocker does. */
function guarded(target: string, go: () => void): void {
  const to = resolve(target);
  for (const b of blockers) {
    if (b.shouldBlock(to)) {
      b.hold(go);
      return;
    }
  }
  go();
}

const router: Navigation = {
  // A navigation writes the harness history (via `window.history`), after any active leave guard lets it through.
  push: (href, options) => {
    installHistory();
    const go = () => {
      window.history.pushState(null, "", href);
    };
    if (options?.ignoreBlocker) go();
    else guarded(href, go);
  },
  replace: (href, options) => {
    installHistory();
    const go = () => {
      window.history.replaceState(null, "", href);
    };
    if (options?.ignoreBlocker) go();
    else guarded(href, go);
  },
  back: () => {
    if (history.index === 0) return;
    guarded(history.entries[history.index - 1] ?? "/", back);
  },
  forward: () => {
    if (history.index >= history.entries.length - 1) return;
    guarded(history.entries[history.index + 1] ?? "/", forward);
  },
  prefetch: () => undefined,
  refresh: () => undefined,
};

/**
 * The harness's leave blocker, with the rules of the app's (router-provider.tsx): while `when`, hold a move to another
 * URL except /login, and, with `samePath`, except a search-only move. Back/Forward through the port are held too.
 */
function useHarnessLeaveBlocker({ when, samePath }: LeaveBlockerOptions): LeaveBlocker {
  const [pending, setPending] = useState<{ go: () => void } | null>(null);
  useEffect(() => {
    if (!when) return;
    const blocker: HarnessBlocker = {
      shouldBlock: (target) => {
        const next = parse(target);
        const current = routerState();
        if (target === current.href) return false;
        if (next.pathname === "/login") return false;
        return !(samePath && next.pathname === current.pathname);
      },
      hold: (go) => {
        setPending({ go });
      },
    };
    blockers.add(blocker);
    return () => {
      blockers.delete(blocker);
    };
  }, [when, samePath]);
  return {
    blocked: pending !== null,
    proceed: () => {
      const held = pending;
      setPending(null);
      held?.go();
    },
    reset: () => {
      setPending(null);
    },
  };
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

/** The provider alone, for tests that render their own tree (`renderWithRouter` wraps it around `ui`). */
export function RouterHarness({ children }: { children: ReactNode }) {
  const loc = useSyncExternalStore(subscribe, routerState, routerState);
  useEffect(() => {
    const onClick = (e: MouseEvent) => {
      if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey)
        return;
      const anchor = (e.target as Element | null)?.closest("a[href]");
      if (!(anchor instanceof HTMLAnchorElement) || anchor.target) return;
      const href = anchor.getAttribute("href") ?? "";
      if (!href.startsWith("/") || href.startsWith("//")) return;
      e.preventDefault();
      router.push(href);
    };
    document.addEventListener("click", onClick);
    return () => {
      document.removeEventListener("click", onClick);
    };
  }, []);
  return (
    <NavigationContext
      value={{
        router,
        pathname: loc.pathname,
        search: loc.search.toString(),
        hash: parse(loc.href).hash.replace(/^#/, ""),
      }}
    >
      <LeaveBlockerContext value={useHarnessLeaveBlocker}>{children}</LeaveBlockerContext>
    </NavigationContext>
  );
}

export interface RenderWithRouterOptions extends Omit<RenderOptions, "wrapper"> {
  /** The starting URL (path, query and hash), default "/". */
  url?: string | undefined;
}

/** Render `ui` at `url`; `router` is the instance pages get from `useRouter()`. */
export function renderWithRouter(
  ui: ReactElement,
  { url = "/", ...options }: RenderWithRouterOptions = {},
): RenderResult & { router: Navigation } {
  installHistory();
  history.entries = [url.startsWith("/") ? url : `/${url}`];
  history.states = [stamp(null, 0)];
  history.index = 0;
  snapshot = null;
  syncHash();
  const result = render(ui, { ...options, wrapper: RouterHarness });
  return { ...result, router };
}
