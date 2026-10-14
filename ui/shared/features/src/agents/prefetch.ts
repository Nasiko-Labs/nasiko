// Ported from nasiko-ui-lab@1b99bdda common/src/features/agents/prefetch.ts. Local edits: each prefetch takes the
// product `scope` the keys carry (the lab's keys were unscoped); `meQuery` is `meQueryOptions(scope)`; the lab's route
// `loader({ preload })` called these on link intent, here a link's hover/focus handler may call them (pilot finding 6:
// the port's links also preload the route's code); `prefetchQuery`/`prefetchInfiniteQuery` (deprecated in this TanStack
// Query) are `query(options)` / `infiniteQuery(options)` with the error swallowed, the same fetch-if-stale; Prettier
// formatting.
/** Route preload prefetches (plan §8 Phase 3): hovering a link starts the page's first request, under the page's own key. */
import type { QueryClient } from "@tanstack/react-query";
import type { Scope } from "@nasiko/data-access";
import { meQueryOptions, type Me } from "@nasiko/data-access/product";
import { agentDetailQuery, catalogQuery, directoryQuery, ownedQuery } from "./api";
import { isUuid } from "./normalize";
import type { MineSearch } from "./search";

const noop = () => undefined;

export const prefetchCatalog = (client: QueryClient, scope: Scope) =>
  void client.infiniteQuery(catalogQuery(scope)).catch(noop);

/** Your agents: the owner is the signed-in user (the _app guard loaded `me`), or a superuser's pick. */
export function prefetchMine(client: QueryClient, scope: Scope, search: Pick<MineSearch, "owner">) {
  const me = client.getQueryData<Me>(meQueryOptions(scope).queryKey);
  const owner = (me?.is_superuser && search.owner) || me?.sub;
  if (owner) void client.query(ownedQuery(scope, owner)).catch(noop);
}

/** A UUID loads the detail; a name resolves through the directory first. */
export function prefetchAgent(client: QueryClient, scope: Scope, ref: string) {
  if (isUuid(ref)) void client.query(agentDetailQuery(scope, ref.toLowerCase())).catch(noop);
  else void client.query(directoryQuery(scope)).catch(noop);
}
