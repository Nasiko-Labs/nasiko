// Ported from nasiko-ui-lab@1b99bdda common/src/test/renderApp.tsx. Local edits: the TanStack router with a memory
// history becomes the navigation-port test harness (@nasiko/testing/next-router) around the workspace route table
// (tooling/routes/routes.json → ./routes.gen.tsx): the URL picks the route component as the route table would,
// inside the signed-in providers (the server layout's guard is not part of a jsdom render, so the principal is the
// seed admin unless `me` is given), or the bare sign-in tree on /login. The edition is a parameter (OSS by default;
// an Enterprise test passes its edition). The returned `router` is a small facade over the harness with the parts of
// TanStack's router the lab's page tests use: `navigate({ to, params, search, replace, state })` (`$name` segments of `to`
// filled from `params`, encoded as TanStack encoded them), `state.location` (`pathname`, and
// `search` parsed as TanStack does: each value JSON-parsed when it can be, so `?expired=true` reads `true`, and `hash`
// without its `#`), and
// `history` (`push`, `back`, `forward`, `length`). The query client is captured from the tree. The matched route's
// dynamic segments are provided as path params (`PathParamsContext`), so a route component's `useParams()`
// reads them decoded, as the app's router gives them. Queries don't retry, as in the lab's
// `renderApp` (`createQueryClient(…, { retry: false })`). The route table's server-side kinds are replayed: a
// `redirect` route replaces the URL with its target and a `superuser` route sends a non-superuser (the re-read
// `/api/me`) to its `to`, as the server guard does (the lab's `beforeLoad` redirects ran in its router). An edition's
// own routes (EE: Access control) are not in the public map: its tests pass them as `routes` (the edition's generated
// map, tooling/routes/generate.mjs), so this harness never imports enterprise code.
import { useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { act, type RenderResult } from "@testing-library/react";
import { usePathname, useRouter, useSearchParams } from "@nasiko/app-core/navigation";
import { PathParamsContext, type HistoryState } from "@nasiko/app-core/navigation";
import { createElement, Suspense, useEffect, type ComponentType } from "react";
import * as z from "zod/mini";
import { OSS, type Edition } from "@nasiko/app-core/edition/edition";
import { EditionProvider } from "@nasiko/app-core/edition/EditionProvider";
import { meQueryOptions, useProductScope, type Me } from "@nasiko/data-access/product";
import { ADMIN_ID } from "@nasiko/mocks/seed-harness";
import { navigate as harnessNavigate, renderWithRouter, routerState } from "@nasiko/testing/router";
import { LoginPage } from "../login/login-page";
import { LoginProviders } from "../login/login-providers";
import { loginSearchSchema } from "../login/search";
import { NotFound } from "../shell/NotFound";
import { AppProviders } from "../workspace/app-providers";
import { TEST_ROUTES, type TestRoute } from "./routes.gen";

export const ADMIN_ME: Me = { sub: ADMIN_ID, username: "admin", is_superuser: true };
export const TEST_SCOPE = { workspaceId: "local", epoch: 0 } as const;

/** `/agents/[agentId]` → a matcher; `[[...rest]]` and a trailing `*` match any rest. */
function matcher(pattern: string): (pathname: string) => boolean {
  const src = pattern
    .split("/")
    .map((seg) => {
      if (seg === "*" || /^\[\[\.\.\.[^\]]+\]\]$/.test(seg)) return "(?:/.*)?";
      if (/^\[\.\.\.[^\]]+\]$/.test(seg)) return "/.+";
      if (/^\[[^\]]+\]$/.test(seg)) return "/[^/]+";
      return seg ? `/${seg.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}` : "";
    })
    .join("");
  const re = new RegExp(`^${src || "/"}/?$`);
  return (p) => re.test(p);
}

type MatchedRoute = TestRoute & { match: (pathname: string) => boolean };
const withMatcher = (r: TestRoute): MatchedRoute => ({ ...r, match: matcher(r.path) });
const ROUTES = TEST_ROUTES.map(withMatcher);

/** The dynamic segments of `pattern` in `pathname` (`/agents/[agentId]` at `/agents/x` → `{ agentId: "x" }`). */
export function paramsFor(pattern: string, pathname: string): Record<string, string | string[]> {
  const pat = pattern.split("/");
  const got = pathname.split("/");
  const out: Record<string, string | string[]> = {};
  const dec = (v: string) => {
    try {
      return decodeURIComponent(v);
    } catch {
      return v;
    }
  };
  pat.forEach((seg, i) => {
    const rest = /^\[\[?\.\.\.([^\]]+)\]\]?$/.exec(seg);
    if (rest?.[1]) {
      const vals = got.slice(i).filter(Boolean).map(dec);
      if (vals.length) out[rest[1]] = vals;
      return;
    }
    const one = /^\[([^\]]+)\]$/.exec(seg);
    if (one?.[1] && got[i] !== undefined) out[one[1]] = dec(got[i]);
  });
  return out;
}

export function routeFor(
  pathname: string,
  edition: Edition = OSS,
  extra: readonly MatchedRoute[] = [],
): ComponentType | null {
  return matchRoute(pathname, edition, extra)?.Component ?? null;
}

function matchRoute(pathname: string, edition: Edition, extra: readonly MatchedRoute[]) {
  // Static segments win over dynamic ones, as in the app's router: try the routes with fewer brackets first.
  const ranked = [...ROUTES, ...extra]
    .filter((r) => r.editions.includes(edition.id === "oss" ? "oss" : "ee"))
    .sort((a, b) => (a.path.match(/\[|\*/g)?.length ?? 0) - (b.path.match(/\[|\*/g)?.length ?? 0));
  return ranked.find((r) => r.match(pathname)) ?? null;
}

const captured: { current: QueryClient | null } = { current: null };

function Capture() {
  const client = useQueryClient();
  useEffect(() => {
    captured.current = client;
  }, [client]);
  return null;
}

function App({ me, edition, extra }: { me: Me; edition: Edition; extra: readonly MatchedRoute[] }) {
  const pathname = usePathname();
  const params = useSearchParams();
  if (pathname === "/login") {
    const raw: Record<string, string> = {};
    params.forEach((v, k) => {
      raw[k] = v;
    });
    return (
      <LoginProviders>
        <Capture />
        <LoginPage search={z.parse(loginSearchSchema, raw)} />
      </LoginProviders>
    );
  }
  const route = matchRoute(pathname, edition, extra);
  return (
    <AppProviders me={me} scope={TEST_SCOPE} initialOpen={undefined} retry={false}>
      <Capture />
      <PathParamsContext.Provider value={route ? paramsFor(route.path, pathname) : null}>
        {/* The route table's components are module constants (routes.gen.tsx), so the element type is stable. */}
        <Suspense fallback={null}>{route ? <RouteElement route={route} /> : <NotFound />}</Suspense>
      </PathParamsContext.Provider>
    </AppProviders>
  );
}

/**
 * The matched route as the server would serve it: a `redirect` route replaces the URL with its target (a
 * replace, as the app's redirect routes do), a `superuser` route replays `superuserOr(to)` against the signed-in principal (the
 * re-checked `/api/me`, which the mocks' `superuser` setting answers) before its component renders.
 */
function RouteElement({ route }: { route: TestRoute }) {
  if (route.redirect !== undefined) return <Redirect to={route.redirect} />;
  if (!route.Component) return <NotFound />;
  if (route.superuserOr !== undefined)
    return <SuperuserOr to={route.superuserOr} Component={route.Component} />;
  return createElement(route.Component);
}

function Redirect({ to }: { to: string }) {
  const router = useRouter();
  useEffect(() => {
    router.replace(to);
  }, [router, to]);
  return null;
}

function SuperuserOr({ to, Component }: { to: string; Component: ComponentType }) {
  const router = useRouter();
  const me = useQuery(meQueryOptions(useProductScope()));
  // The seeded principal is stale (updatedAt 0) and the guard re-reads it on mount: decide on the server's answer.
  const settled = me.dataUpdatedAt > 0 || !me.isFetching;
  const denied = settled && me.data?.is_superuser !== true;
  useEffect(() => {
    if (denied) router.replace(to);
  }, [denied, router, to]);
  return settled && !denied ? createElement(Component) : null;
}

/** TanStack's default search parsing: each value JSON-parsed when it can be. */
function parseSearch(params: URLSearchParams): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  params.forEach((v, k) => {
    try {
      out[k] = JSON.parse(v);
    } catch {
      out[k] = v;
    }
  });
  return out;
}

function hrefOf(to: string, search?: Record<string, unknown>): string {
  if (!search) return to;
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(search)) {
    if (v === undefined) continue;
    q.set(k, typeof v === "string" ? v : JSON.stringify(v));
  }
  const s = q.toString();
  return s ? `${to}?${s}` : to;
}

export interface TestRouter {
  navigate(opts: {
    to: string;
    params?: Record<string, string>;
    search?: Record<string, unknown>;
    replace?: boolean;
    /** The entry's history state, or an updater over the current one (TanStack's `navigate({ state })`). */
    state?: HistoryState | ((prev: HistoryState) => HistoryState);
  }): Promise<void>;
  readonly state: {
    location: { pathname: string; search: Record<string, unknown>; href: string; hash: string };
  };
  readonly history: {
    push(href: string): void;
    back(): void;
    forward(): void;
    readonly length: number;
  };
}

const testRouter: TestRouter = {
  navigate({ to, params, search, replace, state }) {
    const path = to.replace(/\$(\w+)/g, (m, name: string) => {
      const v = params?.[name];
      return v !== undefined ? encodeURIComponent(v) : m;
    });
    const href = hrefOf(path, search);
    const prev = (window.history.state ?? {}) as HistoryState;
    const next = typeof state === "function" ? state(prev) : state;
    act(() => {
      if (replace) window.history.replaceState(next ?? window.history.state, "", href);
      else if (next) window.history.pushState(next, "", href);
      else harnessNavigate(href);
    });
    return Promise.resolve();
  },
  get state() {
    const s = routerState();
    // TanStack's `location.hash` has no `#`.
    const hash = new URL(s.href, "http://harness.test").hash.replace(/^#/, "");
    return {
      location: { pathname: s.pathname, search: parseSearch(s.search), href: s.href, hash },
    };
  },
  history: {
    push(href: string) {
      act(() => {
        harnessNavigate(href);
      });
    },
    back() {
      act(() => {
        window.history.back();
      });
    },
    forward() {
      act(() => {
        window.history.forward();
      });
    },
    get length() {
      return routerState().length;
    },
  },
};

/** Render the whole workspace app at `url` against the MSW handlers (the lab's `renderApp`). */
export function renderApp(
  url: string,
  {
    me = ADMIN_ME,
    edition = OSS,
    routes = [],
  }: {
    me?: Me;
    edition?: Edition;
    /** An edition's own routes (its generated test map), matched with the public ones. */
    routes?: readonly TestRoute[];
  } = {},
): RenderResult & { router: TestRouter; readonly queryClient: QueryClient } {
  const extra = routes.map(withMatcher);
  const result = renderWithRouter(
    <EditionProvider edition={edition}>
      <App me={me} edition={edition} extra={extra} />
    </EditionProvider>,
    { url },
  );
  return Object.assign(result, {
    router: testRouter,
    get queryClient(): QueryClient {
      if (!captured.current) throw new Error("no query client rendered yet");
      return captured.current;
    },
  });
}
