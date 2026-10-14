// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/mcp.$connectorId.tsx. Local edits: a route component (the SPA's generated route table)
// in place of `createFileRoute('/_app/mcp/$connectorId')`: the id comes from `useParams()` (the `[connectorId]`
// segment, decoded as the lab's router decoded `$connectorId`) and the search from
// `useSearchState(detailSearchSchema)`; `remountDeps: connectorId` is a `key` on the page (another server is another
// page: its tabs and dialogs start fresh); the loader's `preload` prefetch is `prefetchConnector(queryClient, scope,
// id)` (./prefetch.ts), for a link's intent handler; `Suspense` around `useSearchParams` (kept from the Next-era port;
// harmless in the SPA); Prettier formatting.
"use client";

import { useParams } from "@nasiko/app-core/navigation";
import { Suspense } from "react";
import { useSearchState } from "@nasiko/app-core/url-state";
import { DetailPage } from "./DetailPage";
import { detailSearchSchema } from "./search";

export function McpDetailRoute() {
  return (
    <Suspense fallback={null}>
      <Detail />
    </Suspense>
  );
}

function Detail() {
  const { connectorId } = useParams<{ connectorId: string }>();
  const { search } = useSearchState(detailSearchSchema);
  return <DetailPage key={connectorId} id={connectorId} search={search} />;
}
