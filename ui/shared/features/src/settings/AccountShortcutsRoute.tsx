// Ported from the legacy ui/ on development (3aac84f66) common/src/routes/_app/account.shortcuts.tsx (51901c342).
// Local edits: a route component (the SPA's generated route table) in place of `createFileRoute`, shaped like
// ./AccountPasswordRoute.tsx; Prettier formatting.
"use client";

import { Suspense } from "react";
import { SettingsLayout } from "./SettingsLayout";
import { ShortcutsPage } from "./ShortcutsPage";

/** `/account/shortcuts`: every user's own reference, no superuser gate (unlike the workspace sections). */
export function AccountShortcutsRoute() {
  return (
    <Suspense fallback={null}>
      <SettingsLayout account>
        <ShortcutsPage />
      </SettingsLayout>
    </Suspense>
  );
}
