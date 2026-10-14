// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/flows.index.tsx. Local edits: a route component (the SPA's generated route table) in
// place of `createFileRoute('/_app/flows/')`: `validateSearch` is `useSearchState(flowsSearchSchema)` and
// `useSetSearch` its `setSearch` (a push unless `replace`); the loader's `preload` prefetch is `prefetchFlows(
// queryClient, scope, deps)` (./prefetch.ts), for a link's hover/focus intent handler; `Suspense` around
// `useSearchParams` (kept from the Next-era port; harmless in the SPA); Prettier formatting.
"use client";

import { Suspense } from "react";
import { useSearchState } from "@nasiko/app-core/url-state";
import { FlowsPage } from "./FlowsPage";
import { flowsSearchSchema } from "./search";

/** Your flows (plans/feat-flows.md §3). */
export function FlowsRoute() {
  return (
    <Suspense fallback={null}>
      <Flows />
    </Suspense>
  );
}

function Flows() {
  const { search, setSearch } = useSearchState(flowsSearchSchema);
  return <FlowsPage search={search} setSearch={setSearch} />;
}
