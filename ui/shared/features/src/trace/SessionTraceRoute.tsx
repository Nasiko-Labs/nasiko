// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/sessions.$sessionId.tsx. Local edits: a route component (the SPA's generated
// route table) in place of `createFileRoute('/_app/sessions/$sessionId')`: the id comes from `useParams()` (the `[sessionId]`
// segment, decoded as the lab's router decoded `$sessionId`) and the search from `useSearchState(traceSearchSchema)`;
// `useSetSearch(…, true)` is `setSearch` with `replace` defaulting to true (selection replaces history, as in the lab);
// `remountDeps: sessionId` is a `key` on the page (another session is another page: its frozen clock, selection and
// panels start fresh); the loader's `preload` prefetch is `prefetchSessionDetail(queryClient, scope, id)`
// (../sessions/api.ts), for a link's intent handler; `Suspense` around `useSearchParams`; Prettier formatting.
"use client";

import { useParams } from "@nasiko/app-core/navigation";
import { Suspense, useCallback } from "react";
import { useSearchState } from "@nasiko/app-core/url-state";
import { traceSearchSchema, type TraceSearch } from "../sessions/search";
import { SessionTracePage } from "./SessionTracePage";

export function SessionTraceRoute() {
  return (
    <Suspense fallback={null}>
      <Trace />
    </Suspense>
  );
}

function Trace() {
  const { sessionId } = useParams<{ sessionId: string }>();
  const { search, setSearch } = useSearchState(traceSearchSchema);
  const replaceByDefault = useCallback(
    (patch: Partial<TraceSearch>, opts?: { replace?: boolean | undefined }) => {
      setSearch(patch, { replace: opts?.replace ?? true });
    },
    [setSearch],
  );
  return (
    <SessionTracePage
      key={sessionId}
      sessionId={sessionId}
      search={search}
      setSearch={replaceByDefault}
    />
  );
}
