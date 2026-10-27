/** Route preload prefetch: hovering a link to a flow starts its first read, under the page's own key. */
import type { QueryClient } from '@tanstack/react-query'
import { resolveWindow } from '@nasiko/features/tokenops/window'
import { flowQuery, flowsListQuery } from './api'
import type { FlowsSearch } from './search'

export const prefetchFlow = (client: QueryClient, id: string) =>
  void client.prefetchQuery(flowQuery(id))

/** The list's first read, keyed as the page keys it (its window resolves against the same minute). */
export function prefetchFlows(
  client: QueryClient,
  s: Pick<FlowsSearch, 'preset' | 'from' | 'to' | 'q' | 'status'>,
) {
  const win = resolveWindow({ preset: s.preset, from: s.from, to: s.to }, new Date())
  void client.prefetchQuery(
    flowsListQuery({
      status: s.status === 'all' ? undefined : s.status,
      q: s.q,
      sinceMs: win.start.getTime(),
    }),
  )
}
