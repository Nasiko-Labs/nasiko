/** Route preload prefetches: hovering a link starts the page's first requests, under the page's own keys. */
import type { QueryClient } from '@tanstack/react-query'
import { executionQuery, executionsQuery, workflowListQuery, workflowQuery } from './api'
import type { DraftSort, WorkflowSort } from './search'

export const prefetchDeployed = (client: QueryClient, sort: WorkflowSort = 'recent') =>
  void client.prefetchQuery(workflowListQuery({ mode: 'deployed', sort }))

export const prefetchDrafts = (client: QueryClient, sort: DraftSort = 'all') =>
  void client.prefetchQuery(workflowListQuery({ mode: 'drafts', sort }))

export const prefetchRuns = (client: QueryClient) => void client.prefetchQuery(executionsQuery)

export function prefetchWorkflow(client: QueryClient, id: string, run?: string) {
  void client.prefetchQuery(workflowQuery(id))
  if (run) void client.prefetchQuery(executionQuery(run))
}
