// Ported from nasiko-ui-lab@1b99bdda common/src/app/mount.tsx (the query client and `watchOtherTabs` around the
// router, as they apply to the sign-in page). Local edits: a client component for the bare `/login` page only
// (the signed-in shell has its own in app/(app)/app-providers.tsx); the 401 reaction is a no-op here (the lab's
// stood down on /login too); the watcher has no `me` to compare (`recheck` is a no-op, the key is unscoped) since
// a sign-in elsewhere sends this tab on to its redirect anyway; Prettier formatting.
"use client";

import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { useEffect, useState, type ReactNode } from "react";
import { createProductQueryClient } from "@nasiko/data-access/product";
import { watchOtherTabs, windowLocation } from "../shell/sessionSync";

export function LoginProviders({ children }: { children: ReactNode }) {
  const [queryClient] = useState<QueryClient>(() =>
    createProductQueryClient({ onUnauthorized: () => undefined }),
  );
  useEffect(
    () =>
      watchOtherTabs({
        queryClient,
        meKey: ["login", "me"],
        location: windowLocation,
        recheck: () => undefined,
      }),
    [queryClient],
  );
  return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
}
