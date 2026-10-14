// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/index.tsx. Local edits: a route component (the SPA's generated route table) in place of
// `createFileRoute('/_app/')`: `validateSearch` is `useSearchState(overviewSearchSchema)` and `useSetSearch(…, true)`
// its `setSearch` with `replace: true` (the range replaces history, as the lab's did); `Suspense` around
// `useSearchParams` (kept from the Next-era port; harmless in the SPA); Prettier formatting.
"use client";

import { Suspense } from "react";
import { useSearchState } from "@nasiko/app-core/url-state";
import { useGuide, useGuideState } from "../onboarding/api";
import { GuideHost } from "../onboarding/GuideHost";
import { OverviewPage } from "./OverviewPage";
import { overviewSearchSchema, type OverviewSearch } from "./search";

/** The Overview homepage, or the onboarding guide in its place for a first-time user (spec §2). */
export function OverviewRoute() {
  return (
    <Suspense fallback={null}>
      <Overview />
    </Suspense>
  );
}

function Overview() {
  const { search, setSearch } = useSearchState(overviewSearchSchema);
  const { due } = useGuide();
  const state = useGuideState();
  // A first-time user gets the guide instead of the Overview (spec §2); skipping or finishing shows it.
  const firstRun = due || (state.open && state.firstRun);
  return (
    <>
      {firstRun ? null : (
        <OverviewPage
          search={search}
          setSearch={(patch: Partial<OverviewSearch>) => {
            setSearch(patch, { replace: true });
          }}
        />
      )}
      <GuideHost />
    </>
  );
}
