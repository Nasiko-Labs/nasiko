// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/harnesses.tsx. Local edits: a route component (the SPA's generated route table) in
// place of `createFileRoute('/_app/harnesses')`: `validateSearch` is `useSearchState(harnessesSearchSchema)` (the
// schema is loose, so an edition page's own keys pass through the URL untouched) and `useSetSearch` its `setSearch`;
// `Suspense` around `useSearchParams` (kept from the Next-era port; harmless in the SPA); Prettier formatting.
"use client";

import { Suspense } from "react";
import { useSlots } from "@nasiko/app-core/edition/edition-context";
import { useSearchState } from "@nasiko/app-core/url-state";
import { HarnessesPage } from "./HarnessesPage";
import { harnessesSearchSchema } from "./search";

export function HarnessesRoute() {
  return (
    <Suspense fallback={null}>
      <Harnesses />
    </Suspense>
  );
}

/** A layer that serves the org levels (EE) renders its own page here; the core shows the viewer's own usage. */
function Harnesses() {
  const OrgPage = useSlots().harnessOrgScope?.Page;
  return OrgPage ? <OrgPage /> : <CorePage />;
}

function CorePage() {
  const { search, setSearch } = useSearchState(harnessesSearchSchema);
  return <HarnessesPage search={search} setSearch={setSearch} />;
}
