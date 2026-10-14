// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/deploy.tsx. Local edits: a route component (the SPA's generated route table) in place
// of `createFileRoute('/_app/deploy')`: `validateSearch` is `useSearchState(deploySearchSchema)` and the route's
// `navigate({ search: (prev) => ({ ...prev, ...patch }), replace: true, resetScroll: false })` is its `setSearch(patch,
// { replace: true })` (the History API, never a scroll); `Suspense` around `useSearchParams` (kept from the Next-era port;
// harmless in the SPA); Prettier formatting.
"use client";

import { Suspense, useCallback } from "react";
import { useSearchState } from "@nasiko/app-core/url-state";
import { DeployPage } from "./DeployPage";
import { deploySearchSchema, type DeploySearch } from "./search";

export function DeployRoute() {
  return (
    <Suspense fallback={null}>
      <Deploy />
    </Suspense>
  );
}

function Deploy() {
  const { search, setSearch: set } = useSearchState(deploySearchSchema);
  // The method tab and the picked repository replace history (they are views, not steps).
  const setSearch = useCallback(
    (patch: Partial<DeploySearch>) => {
      set(patch, { replace: true });
    },
    [set],
  );
  return <DeployPage search={search} setSearch={setSearch} />;
}
