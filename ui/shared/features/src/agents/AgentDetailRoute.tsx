// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/agents.$agentId.tsx. Local edits: a route component (the SPA's generated route table)
// in place of `createFileRoute('/_app/agents/$agentId')`: the ref comes from `useParams()` (the `[agentId]` segment,
// decoded as the lab's router decoded `$agentId`) and `tab` from `useSearchState(hubSearchSchema)`; `remountDeps:
// agentId` is a `key` on the page (another agent is another page: its tabs, dialogs and watches start fresh); the
// loader's `preload` prefetch is `prefetchAgent(queryClient, scope, ref)` (./prefetch.ts), for a link's intent
// handler; `Suspense` around `useSearchParams` (kept from the Next-era port; harmless in the SPA); Prettier
// formatting.
"use client";

import { useParams } from "@nasiko/app-core/navigation";
import { Suspense } from "react";
import { useSearchState } from "@nasiko/app-core/url-state";
import { AgentDetailPage } from "./AgentDetailPage";
import { hubSearchSchema } from "../hub/search";

export function AgentDetailRoute() {
  return (
    <Suspense fallback={null}>
      <Detail />
    </Suspense>
  );
}

function Detail() {
  const { agentId } = useParams<{ agentId: string }>();
  const { search } = useSearchState(hubSearchSchema);
  return <AgentDetailPage key={agentId} agentRef={agentId} tab={search.tab} />;
}
