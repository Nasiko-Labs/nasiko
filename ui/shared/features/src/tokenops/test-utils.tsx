// Ported from nasiko-ui-lab@1b99bdda common/src/test/renderApp.tsx (the TokenOps half). Local edits: renders the
// TokenOps route page under `ProductQueryProvider` (the mock principal's scope) and the navigation-port test harness (`@nasiko/testing/router`)
// (./next-router.tsx), not the whole app (the shell, its guard and /login are the shell's own tests); the query
// client is `createProductQueryClient({ onUnauthorized, retry: false })` with `onUnauthorized` a spy the 401 tests
// read (the lab asserted the router's redirect); the harness router is wrapped so every push/replace/back stamps
// the entry's position into `window.history.state` (`HISTORY_INDEX_KEY`, lib/url-state.ts), which is what the page
// reads to decide Back-vs-replace on close — the lab's `__TSR_index`; `location()` is the lab's
// `router.state.location` (search parsed like the lab's router: `true`/`false` and numbers as values);
// `MotionConfig`/`LazyMotion`/`LayoutGroup` dropped (the motion runtime is foundation-only); Prettier formatting.
import type { QueryClient } from "@tanstack/react-query";
import { createProductQueryClient, ProductQueryProvider } from "@nasiko/data-access/product";
import { NavigationContext } from "@nasiko/app-core/navigation";
import { useContext, useMemo, type ReactNode } from "react";
import { vi, type Mock } from "vitest";
import { TokenopsRoutePage } from "./TokenopsRoute";
import { HISTORY_INDEX_KEY } from "@nasiko/app-core/url-state";
import { renderWithRouter, routerState } from "@nasiko/testing/router";

/** The mock principal's scope (server/identity/mock-delegation.ts: workspace `local`, epoch 0). */
export const MOCK_SCOPE = { workspaceId: "local", epoch: 0 } as const;

export interface TestLocation {
  pathname: string;
  /** The query, parsed the way the lab's router parsed it (`?traces=true` -> `true`, `?compare=0` -> `0`). */
  search: Record<string, unknown>;
  /** pathname + query. */
  href: string;
  /** The entry's position in the history (the lab's `__TSR_index`). */
  index: number;
  /** The lab's `router.history.length`. */
  length: number;
}

function parseValue(v: string): unknown {
  if (v === "true") return true;
  if (v === "false") return false;
  if (/^-?\d+(\.\d+)?$/.test(v)) return Number(v);
  return v;
}

/** The current entry, as the lab's tests read `router.state.location`. */
export function location(): TestLocation {
  const s = routerState();
  const search: Record<string, unknown> = {};
  for (const [k, v] of s.search.entries()) search[k] = parseValue(v);
  return { pathname: s.pathname, search, href: s.href, index: s.index, length: s.length };
}

function stampIndex(): void {
  window.history.replaceState({ [HISTORY_INDEX_KEY]: routerState().index }, "");
}

export interface TestRouter {
  push: (href: string) => void;
  replace: (href: string) => void;
  back: () => void;
  forward: () => void;
  /** The lab's `router.navigate({ to: "/tokenops", search })`: a push of the whole search. */
  navigate: (
    search: Record<string, unknown> | ((prev: Record<string, unknown>) => Record<string, unknown>),
  ) => void;
}

/** The harness router, each move stamping the new position into `window.history.state`. */
function Stamping({ children, expose }: { children: ReactNode; expose: (r: TestRouter) => void }) {
  const context = useContext(NavigationContext);
  const harness = context?.router;
  const router = useMemo(() => {
    if (!harness) throw new Error("renderTokenops: no router harness");
    const after = <A extends unknown[]>(fn: (...a: A) => void) => {
      return (...a: A) => {
        fn(...a);
        stampIndex();
      };
    };
    const stamped = {
      ...harness,
      push: after((href: string) => {
        harness.push(href);
      }),
      replace: after((href: string) => {
        harness.replace(href);
      }),
      back: after(() => {
        harness.back();
      }),
      forward: after(() => {
        harness.forward();
      }),
    };
    expose({
      push: stamped.push,
      replace: stamped.replace,
      back: stamped.back,
      forward: stamped.forward,
      navigate: (search) => {
        const next = typeof search === "function" ? search(location().search) : search;
        const qs = new URLSearchParams();
        for (const [k, v] of Object.entries(next))
          if (typeof v === "string" || typeof v === "number" || typeof v === "boolean")
            qs.set(k, String(v));
        const s = qs.toString();
        stamped.push(s ? `/tokenops?${s}` : "/tokenops");
      },
    });
    return stamped;
  }, [harness, expose]);
  return (
    <NavigationContext.Provider value={context ? { ...context, router } : null}>
      {children}
    </NavigationContext.Provider>
  );
}

export interface RenderedTokenops {
  router: TestRouter;
  queryClient: QueryClient;
  /** Called by the query client on a 401 burst (the lab navigated to `/login?expired=true` here). */
  onUnauthorized: Mock<() => void | Promise<void>>;
}

export function renderTokenops(url: string): RenderedTokenops {
  const onUnauthorized = vi.fn<() => void | Promise<void>>();
  const queryClient = createProductQueryClient({ onUnauthorized, retry: false });
  // The entry list starts over at `url`: its position is 0 whatever the previous test left in jsdom's history.
  window.history.replaceState({ [HISTORY_INDEX_KEY]: 0 }, "");
  let exposed: TestRouter | undefined;
  const expose = (r: TestRouter) => {
    exposed = r;
  };
  renderWithRouter(
    <Stamping expose={expose}>
      <ProductQueryProvider scope={MOCK_SCOPE} queryClient={queryClient}>
        <TokenopsRoutePage />
      </ProductQueryProvider>
    </Stamping>,
    { url },
  );
  stampIndex();
  if (!exposed) throw new Error("renderTokenops: the router harness did not mount");
  return { router: exposed, queryClient, onUnauthorized };
}
