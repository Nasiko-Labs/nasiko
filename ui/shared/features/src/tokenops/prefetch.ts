// Ported from nasiko-ui-lab@1b99bdda common/src/features/tokenops/prefetch.ts. Local edits: takes the product
// `scope` the keys carry (the lab's keys were unscoped); the lab's route `loader({ preload })` called it on link
// intent, here the shell's TokenOps link calls `prefetchTokenops(queryClient, scope, deps)` on hover/focus;
// `prefetchQuery` (deprecated in this TanStack Query) is `query(options).catch(noop)`, the same fetch-if-stale and
// swallowed error; Prettier formatting.
import type { QueryClient } from "@tanstack/react-query";
import type { Scope } from "@nasiko/data-access";
import {
  calendarQuery,
  dashboardQuery,
  providersQuery,
  timeseriesQuery,
  type Filters,
} from "./api";
import type { TokenopsSearch } from "./search";
import { monthKey, resolveWindow, utcMonthStart } from "./window";

const noop = () => undefined;

export type TokenopsDeps = Pick<
  TokenopsSearch,
  "preset" | "from" | "to" | "view" | "agent" | "provider" | "model"
>;

/**
 * The route's preload prefetch (plan §8 Phase 3): hovering a TokenOps link starts every query the page's first
 * paint waits on, so the panels still fill together. Same keys
 * as the page, so the page reuses them. The window is resolved from the clock here and again, frozen, by the page;
 * both floor to the minute, so they share one key.
 * ponytail: a hover in one minute and a click in the next fetches that window twice; move the frozen "now" into
 * the route (loaderDeps + invalidate on return) if that ever shows up in request logs.
 */
export function prefetchTokenops(
  client: QueryClient,
  scope: Scope,
  deps: TokenopsDeps,
  now = new Date(),
) {
  const win = resolveWindow(deps, now);
  const f: Filters = { agent: deps.agent, provider: deps.provider, model: deps.model };
  void client.query(dashboardQuery(scope, win, f, deps.view, "current")).catch(noop);
  void client.query(dashboardQuery(scope, win, f, deps.view, "previous")).catch(noop);
  void client.query(timeseriesQuery(scope, win, f)).catch(noop);
  void client.query(calendarQuery(scope, monthKey(utcMonthStart(now)), f)).catch(noop);
  void client.query(calendarQuery(scope, monthKey(utcMonthStart(now, -1)), f)).catch(noop);
  void client.query(providersQuery(scope)).catch(noop);
}
