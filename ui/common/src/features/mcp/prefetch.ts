/** Route preload prefetches: hovering a link starts the page's first requests, under the page's own keys. */
import type { QueryClient } from '@tanstack/react-query'
import { connectorQuery, connectorsQuery, toolkitsQuery } from './api'

export function prefetchCatalog(client: QueryClient) {
  void client.prefetchQuery(connectorsQuery)
  void client.prefetchQuery(toolkitsQuery)
}

export const prefetchConnector = (client: QueryClient, id: string) =>
  void client.prefetchQuery(connectorQuery(id))
