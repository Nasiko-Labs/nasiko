/** Route preload prefetches (plan §8 Phase 3): hovering a link starts the page's first request, under the page's own key. */
import type { QueryClient } from '@tanstack/react-query'
import { meQuery } from '@/lib/api/auth'
import { agentDetailQuery, catalogQuery, directoryQuery, ownedQuery } from './api'
import { isUuid } from './normalize'
import type { MineSearch } from './search'

export const prefetchCatalog = (client: QueryClient) =>
  void client.prefetchInfiniteQuery(catalogQuery)

/** Your agents: the owner is the signed-in user (the _app guard loaded `me`), or a superuser's pick. */
export function prefetchMine(client: QueryClient, search: Pick<MineSearch, 'owner'>) {
  const me = client.getQueryData(meQuery.queryKey)
  const owner = (me?.is_superuser && search.owner) || me?.sub
  if (owner) void client.prefetchQuery(ownedQuery(owner))
}

/** A UUID loads the detail; a name resolves through the directory first. */
export function prefetchAgent(client: QueryClient, ref: string) {
  if (isUuid(ref)) void client.prefetchQuery(agentDetailQuery(ref.toLowerCase()))
  else void client.prefetchQuery(directoryQuery)
}
