// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/tokenops.tsx. Local edits: a route component (the SPA's generated route table) in
// place of `createFileRoute('/_app/tokenops')`: `validateSearch` is `useSearchState(tokenopsSearchSchema)` and
// `useSetSearch` its `setSearch` (lib/url-state.ts, which also lends the page `back` and `historyIndex`); the route
// has no `remountDeps`, so nothing is keyed; the loader's `preload` prefetch is `prefetchTokenops(queryClient, scope,
// deps)` (features/tokenops/prefetch.ts), for the shell's TokenOps link to call on hover/focus intent; `Suspense`
// around `useSearchParams` (kept from the Next-era port; harmless in the SPA); Prettier formatting.
"use client";

import { Suspense } from "react";
import { tokenopsSearchSchema } from "./search";
import { TokenopsPage } from "./TokenopsPage";
import { useSearchState } from "@nasiko/app-core/url-state";

export function TokenopsRoutePage() {
  return (
    <Suspense fallback={null}>
      <TokenopsRoute />
    </Suspense>
  );
}

function TokenopsRoute() {
  const { search, setSearch, back, historyIndex } = useSearchState(tokenopsSearchSchema);
  return (
    <TokenopsPage search={search} setSearch={setSearch} history={{ back, index: historyIndex }} />
  );
}
