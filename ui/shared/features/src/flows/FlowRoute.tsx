// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/flows.$flowId.tsx. Local edits: a route component (the SPA's generated route table) in
// place of `createFileRoute('/_app/flows/$flowId')`: the id comes from `useParams()` (the `[flowId]` segment, decoded as
// the lab's router decoded `$flowId`) and `step` from `useSearchState(flowSearchSchema)`; the selection replaces
// history (`setSearch(…, { replace: true })`, the lab's `useSetSearch(…, true)`); `remountDeps: flowId` is a `key` on
// the page (another flow is another page: its clock, selection and announcements start fresh); the loader's `preload`
// prefetch is `prefetchFlow(queryClient, scope, id)` (./prefetch.ts), for a link's intent handler; `Suspense` around
// `useSearchParams`; Prettier formatting.
"use client";

import { useParams } from "@nasiko/app-core/navigation";
import { Suspense } from "react";
import { useSearchState } from "@nasiko/app-core/url-state";
import { FlowPage } from "./FlowPage";
import { flowSearchSchema } from "./search";

/** One flow (plans/feat-flows.md §2): the id is the flow's trace id; `?step=` is the selected call (F25). */
export function FlowRoute() {
  return (
    <Suspense fallback={null}>
      <Flow />
    </Suspense>
  );
}

function Flow() {
  const { flowId } = useParams<{ flowId: string }>();
  const { search, setSearch } = useSearchState(flowSearchSchema);
  return (
    <FlowPage
      key={flowId}
      flowId={flowId}
      step={search.step ?? null}
      onStep={(step) => {
        // Selection replaces history (F25): Back leaves the flow, it doesn't walk the clicks.
        setSearch({ step: step ?? undefined }, { replace: true });
      }}
    />
  );
}
