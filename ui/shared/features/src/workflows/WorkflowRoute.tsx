// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/workflows.index.tsx, workflows.drafts.tsx,
// workflows.new.tsx, workflows.runs.tsx and workflows.$workflowId.tsx. Local edits: route components (the SPA's generated route table) in place of
// the five `createFileRoute('/_app/workflows/…')` routes, in one module: each `validateSearch` is
// `useSearchState(<schema>)` and `useSetSearch(…, true)` its `setSearch` with `replace: true` (search, sort and the
// runs filters replace history, as the lab's did); the workflow id comes from `useParams()` (the `[workflowId]`
// segment) and `remountDeps: workflowId` is a `key` on the page (another workflow is another page: its editor and
// dialogs start fresh); the loaders' `preload` prefetches are ./prefetch.ts (scoped), for a link's intent handler;
// `Suspense` around `useSearchParams`; Prettier formatting.
// Split from WorkflowsRoute.tsx (stabilization phase): one module per route, so a route's page file loads only its own
// page and not every page the module served (the shared module put all of them in each route's first load).
"use client";

import { useParams } from "@nasiko/app-core/navigation";
import { Suspense } from "react";
import { useSearchState } from "@nasiko/app-core/url-state";
import { workflowSearchSchema } from "./search";
import { WorkflowPage } from "./WorkflowPage";

export function WorkflowRoute() {
  return (
    <Suspense fallback={null}>
      <Workflow />
    </Suspense>
  );
}

function Workflow() {
  const { workflowId } = useParams<{ workflowId: string }>();
  const { search } = useSearchState(workflowSearchSchema);
  return <WorkflowPage key={workflowId} id={workflowId} run={search.run} />;
}
