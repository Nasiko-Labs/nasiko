// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/builds.index.tsx. Local edits: a route component (the SPA's generated route table) in
// place of `createFileRoute('/_app/builds/')`: `validateSearch` is `useSearchState(buildsSearchSchema)` and the
// route's merging `navigate({ search, replace, resetScroll: false })` is its `setSearch(patch, { replace })` (push by
// default, as the lab: paging pushes, filters and search pass `replace`); `Suspense` around `useSearchParams` (kept from the
// Next-era port; harmless in the SPA); Prettier formatting.
"use client";

import { Suspense } from "react";
import { useSearchState } from "@nasiko/app-core/url-state";
import { BuildsPage } from "./BuildsPage";
import { buildsSearchSchema } from "./search";

export function BuildsRoute() {
  return (
    <Suspense fallback={null}>
      <Builds />
    </Suspense>
  );
}

function Builds() {
  // Filters and search replace history; paging pushes it (the TokenOps rule).
  const { search, setSearch } = useSearchState(buildsSearchSchema);
  return <BuildsPage search={search} setSearch={setSearch} />;
}
