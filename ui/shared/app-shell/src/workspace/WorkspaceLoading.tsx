"use client";

// The lab's router-level pending UI (common/src/app/router.tsx: `defaultPendingComponent: RoutePending`,
// `defaultPendingMs: 150`) for the signed-in pages. Local edits: the fallback of the route table's `Suspense`
// (router.tsx), shown while a lazy route's code loads; the 150 ms grace is a timer here (TanStack Router's
// `pendingMs`), so a fast chunk shows nothing rather than a flash. (Orphaned when the Next wrapper was removed, which
// left a bare "Loading…" line in its place; restored 2026-10-13.)
import { useEffect, useState } from "react";
import { RoutePending } from "../shell/RouteStates";

const PENDING_MS = 150;

export function WorkspaceLoading() {
  const [shown, setShown] = useState(false);
  useEffect(() => {
    const t = setTimeout(() => {
      setShown(true);
    }, PENDING_MS);
    return () => {
      clearTimeout(t);
    };
  }, []);
  return shown ? <RoutePending /> : null;
}
