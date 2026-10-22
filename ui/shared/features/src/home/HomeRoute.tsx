// Ported from the legacy ui/ on development (3aac84f66) common/src/routes/_app/index.tsx (the scoped home at `/`,
// be271b564). Local edits: a route component (the SPA's generated route table) in place of
// `createFileRoute('/_app/')`, as ../overview/OverviewRoute.tsx was: `validateSearch` is
// `useSearchState(homeSearchSchema)` and `useSetSearch(…, true)` its `setSearch` with `replace: true`; `Suspense`
// around `useSearchParams`; Prettier formatting.
import { Suspense } from "react";
import { useSearchState } from "@nasiko/app-core/url-state";
import { useGuide, useGuideState } from "../onboarding/api";
import { GuideHost } from "../onboarding/GuideHost";
import { HomePage } from "./HomePage";
import { homeSearchSchema, type HomeSearch } from "./search";

/** The scoped home, or the onboarding guide in its place for a first-time user (spec §2). */
export function HomeRoute() {
  return (
    <Suspense fallback={null}>
      <Home />
    </Suspense>
  );
}

function Home() {
  const { search, setSearch } = useSearchState(homeSearchSchema);
  const { due } = useGuide();
  const state = useGuideState();
  // A first-time user gets the guide instead of the home (spec §2); skipping or finishing shows it.
  const firstRun = due || (state.open && state.firstRun);
  return (
    <>
      {firstRun ? null : (
        <HomePage
          search={search}
          setSearch={(patch: Partial<HomeSearch>) => {
            setSearch(patch, { replace: true });
          }}
        />
      )}
      <GuideHost />
    </>
  );
}
