// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/builds.$buildId.tsx. Local edits: a route component (the SPA's generated route table)
// in place of `createFileRoute('/_app/builds/$buildId')`: the id comes from `useParams()` (the `[buildId]` segment);
// the page is still keyed by it; Prettier formatting.
"use client";

import { useParams } from "@nasiko/app-core/navigation";
import { BuildPage } from "./BuildPage";

/** One build (plans/feat-deploy.md §5): the id is the build id, which equals the upload id. */
export function BuildRoute() {
  const { buildId } = useParams<{ buildId: string }>();
  // Keyed: moving between builds (a toast's See why, Back/Forward) starts from a fresh stream and stage clock.
  return <BuildPage key={buildId} buildId={buildId} />;
}
