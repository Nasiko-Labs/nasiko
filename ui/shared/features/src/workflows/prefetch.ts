// Ported from nasiko-ui-lab@1b99bdda common/src/features/workflows/prefetch.ts. Local edits: the prefetches take the
// scope (scoped keys); TanStack's loader `preload` has no navigation-port counterpart, so these are for a link's intent handler; `prefetchQuery` (deprecated) is `query(…).catch(noop)`;
// Prettier formatting.
/** Route preload prefetches: hovering a link starts the page's first requests, under the page's own keys. */
import type { QueryClient } from "@tanstack/react-query";
import type { Scope } from "@nasiko/data-access";
import { executionQuery, executionsQuery, workflowListQuery, workflowQuery } from "./api";
import type { DraftSort, WorkflowSort } from "./search";

/** A prefetch fails quietly (`prefetchQuery` did): the page's own read shows the error. */
const noop = () => undefined;

export const prefetchDeployed = (
  client: QueryClient,
  scope: Scope,
  sort: WorkflowSort = "recent",
) => void client.query(workflowListQuery(scope, { mode: "deployed", sort })).catch(noop);

export const prefetchDrafts = (client: QueryClient, scope: Scope, sort: DraftSort = "all") =>
  void client.query(workflowListQuery(scope, { mode: "drafts", sort })).catch(noop);

export const prefetchRuns = (client: QueryClient, scope: Scope) =>
  void client.query(executionsQuery(scope)).catch(noop);

export function prefetchWorkflow(client: QueryClient, scope: Scope, id: string, run?: string) {
  void client.query(workflowQuery(scope, id)).catch(noop);
  if (run) void client.query(executionQuery(scope, run)).catch(noop);
}
