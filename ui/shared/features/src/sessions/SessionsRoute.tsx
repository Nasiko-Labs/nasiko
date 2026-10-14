// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/sessions.index.tsx. Local edits: a route component (the SPA's generated route table) in
// place of `createFileRoute('/_app/sessions/')`: `validateSearch` is `useSearchState(sessionsSearchSchema)` and
// `useSetSearch` its `setSearch` (a push unless `replace`, as the lab's); the loader's `preload` prefetch is
// `prefetchSessions(queryClient, scope, deps)` (./api.ts), for a link's hover/focus intent handler; `Suspense` around
// `useSearchParams` (kept from the Next-era port; harmless in the SPA); Prettier formatting.
"use client";

import { Suspense } from "react";
import { useSearchState } from "@nasiko/app-core/url-state";
import { sessionsSearchSchema } from "./search";
import { SessionsPage } from "./SessionsPage";

export function SessionsRoute() {
  return (
    <Suspense fallback={null}>
      <Sessions />
    </Suspense>
  );
}

function Sessions() {
  const { search, setSearch } = useSearchState(sessionsSearchSchema);
  return <SessionsPage search={search} setSearch={setSearch} />;
}
