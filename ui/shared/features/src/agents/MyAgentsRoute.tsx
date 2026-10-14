// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/agents.mine.tsx. Local edits: a route component (the SPA's generated route table) in
// place of `createFileRoute('/_app/agents/mine')`: `validateSearch` is `useSearchState(mineSearchSchema)` and
// `useSetSearch(Route.fullPath, true)` is its `setSearch(patch, { replace: true })`; the loader's `preload` prefetch is
// `prefetchMine(queryClient, scope, { owner })` (./prefetch.ts), for a link's intent handler; `Suspense` around
// `useSearchParams` (kept from the Next-era port; harmless in the SPA); Prettier formatting.
"use client";

import { Suspense, useCallback } from "react";
import { useSearchState } from "@nasiko/app-core/url-state";
import { MyAgentsPage } from "./MyAgentsPage";
import { mineSearchSchema, type MineSearch } from "./search";

export function MyAgentsRoute() {
  return (
    <Suspense fallback={null}>
      <Mine />
    </Suspense>
  );
}

function Mine() {
  const { search, setSearch: set } = useSearchState(mineSearchSchema);
  const setSearch = useCallback(
    (patch: Partial<MineSearch>) => {
      set(patch, { replace: true });
    },
    [set],
  );
  return <MyAgentsPage search={search} setSearch={setSearch} />;
}
