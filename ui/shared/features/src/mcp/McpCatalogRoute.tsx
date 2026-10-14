// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/mcp.index.tsx. Local edits: a route component (the SPA's generated route table) in
// place of `createFileRoute('/_app/mcp/')`: `validateSearch` is `useSearchState(catalogSearchSchema)` and
// `useSetSearch(Route.fullPath, true)` is its `setSearch(patch, { replace: true })` (search, scope and tab replace
// history); the loader's `preload` prefetch is `prefetchCatalog(queryClient, scope)` (./prefetch.ts), for a link's
// intent handler; `Suspense` around `useSearchParams` (kept from the Next-era port; harmless in the SPA); Prettier
// formatting.
"use client";

import { Suspense, useCallback } from "react";
import { useSearchState } from "@nasiko/app-core/url-state";
import { CatalogPage } from "./CatalogPage";
import { catalogSearchSchema, type CatalogSearch } from "./search";

export function McpCatalogRoute() {
  return (
    <Suspense fallback={null}>
      <Catalog />
    </Suspense>
  );
}

function Catalog() {
  const { search, setSearch: set } = useSearchState(catalogSearchSchema);
  // Search, scope and tab replace history: typing must not stack Back entries.
  const setSearch = useCallback(
    (patch: Partial<CatalogSearch>) => {
      set(patch, { replace: true });
    },
    [set],
  );
  return <CatalogPage search={search} setSearch={setSearch} />;
}
