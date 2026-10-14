// Ported from nasiko-ui-lab@1b99bdda common/src/features/mcp/prefetch.ts. Local edits: each prefetch takes the
// product `scope` the keys carry (the lab's keys were unscoped); the lab's route `loader({ preload })` called these on
// link intent, here a link's hover/focus handler may call them (pilot finding 6: the port's links also preload the
// route's code); `prefetchQuery` (deprecated in this TanStack Query) is `query(options)` with the error swallowed, the same
// fetch-if-stale; Prettier formatting.
/** Route preload prefetches: hovering a link starts the page's first requests, under the page's own keys. */
import type { QueryClient } from "@tanstack/react-query";
import type { Scope } from "@nasiko/data-access";
import { connectorQuery, connectorsQuery, toolkitsQuery } from "./api";

const noop = () => undefined;

export function prefetchCatalog(client: QueryClient, scope: Scope) {
  void client.query(connectorsQuery(scope)).catch(noop);
  void client.query(toolkitsQuery(scope)).catch(noop);
}

export const prefetchConnector = (client: QueryClient, scope: Scope, id: string) =>
  void client.query(connectorQuery(scope, id)).catch(noop);
