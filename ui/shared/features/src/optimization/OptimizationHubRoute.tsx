// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/optimization.tsx and settings.optimization-tiers.tsx.
// Local edits: route components (the SPA's generated route table) in place of `createFileRoute`: `validateSearch` is
// `useSearchState(optimizationSearchSchema)` and the lab page's own `useSearch`/`useSetSearch('/optimization')` are its
// `search`/`setSearch`, handed to the page as props (TokenOps' seam); the tiers route's `beforeLoad` superuser redirect
// (to `/optimization#settings`) is the route table's `superuser` guard (tooling/routes/routes.json, run on the server
// before this renders); `Suspense` around `useSearchParams` (kept from the Next-era port; harmless in the SPA); both
// routes share this module; Prettier formatting.
// Split from OptimizationRoute.tsx (stabilization phase): one module per route, so a route's page file loads only its own
// page and not every page the module served (the shared module put all of them in each route's first load).
"use client";

import { Suspense } from "react";
import { useSearchState } from "@nasiko/app-core/url-state";
import { OptimizationHubPage } from "./OptimizationHubPage";
import { optimizationSearchSchema } from "./search";

/** `/optimization`: every user (Observe → Optimization). */
export function OptimizationHubRoute() {
  return (
    <Suspense fallback={null}>
      <Hub />
    </Suspense>
  );
}

function Hub() {
  const { search, setSearch } = useSearchState(optimizationSearchSchema);
  return <OptimizationHubPage search={search} setSearch={setSearch} />;
}
