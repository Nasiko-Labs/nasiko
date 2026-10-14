// Ported from nasiko-ui-lab@1b99bdda common/src/features/flows/prefetch.ts. Local edits: each prefetch takes the
// product `scope` the keys carry; `prefetchQuery` (deprecated in this TanStack Query) is `query(options)` with the
// error swallowed, the same fetch-if-stale; the lab's route `loader({ preload })` called these on link intent, here a
// link's hover/focus handler may call them (pilot finding 6); Prettier formatting.
/** Route preload prefetch: hovering a link to a flow starts its first read, under the page's own key. */
import type { QueryClient } from "@tanstack/react-query";
import type { Scope } from "@nasiko/data-access";
import { resolveWindow } from "../tokenops/window";
import { flowQuery, flowsListQuery } from "./api";
import type { FlowsSearch } from "./search";

const noop = () => undefined;

export const prefetchFlow = (client: QueryClient, scope: Scope, id: string) =>
  void client.query(flowQuery(scope, id)).catch(noop);

/** The list's first read, keyed as the page keys it (its window resolves against the same minute). */
export function prefetchFlows(
  client: QueryClient,
  scope: Scope,
  s: Pick<FlowsSearch, "preset" | "from" | "to" | "q" | "status">,
) {
  const win = resolveWindow({ preset: s.preset, from: s.from, to: s.to }, new Date());
  void client
    .query(
      flowsListQuery(scope, {
        status: s.status === "all" ? undefined : s.status,
        q: s.q,
        sinceMs: win.start.getTime(),
      }),
    )
    .catch(noop);
}
