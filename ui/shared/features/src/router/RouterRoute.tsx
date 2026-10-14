// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/router.tsx. Local edits: a route component (the SPA's generated route table) in place
// of `createFileRoute('/_app/router')`: `validateSearch` is `useSearchState(routerSearchSchema)` and
// `useSetSearch(Route.fullPath, true)` is its `setSearch(patch, { replace: true })` (the source filter and the tab
// replace history); the loader's `preload` prefetch is `prefetchRouter(queryClient, scope)` (./api.ts), for a link's
// intent handler; `Suspense` around `useSearchParams` (kept from the Next-era port; harmless in the SPA); Prettier
// formatting.
"use client";

import { Suspense, useCallback } from "react";
import { useSearchState } from "@nasiko/app-core/url-state";
import { RouterPage } from "./RouterPage";
import { routerSearchSchema, type RouterSearch } from "./search";

export function RouterRoute() {
  return (
    <Suspense fallback={null}>
      <Router />
    </Suspense>
  );
}

function Router() {
  const { search, setSearch: set } = useSearchState(routerSearchSchema);
  // The source filter replaces history, like the catalog's filters.
  const setSearch = useCallback(
    (patch: Partial<RouterSearch>) => {
      set(patch, { replace: true });
    },
    [set],
  );
  return <RouterPage search={search} setSearch={setSearch} />;
}
