// Ported from nasiko-ui-lab@1b99bdda common/src/routes/_app/settings.index.tsx, settings.secrets.tsx,
// account.appearance.tsx and account.password.tsx. Local edits: route components (the SPA's generated route table) in place of
// `createFileRoute`: `validateSearch` is `useSearchState(settingsSearchSchema)` and the lab's
// `useSetSearch(Route.fullPath, true)` its `setSearch` with `replace` by default; `beforeLoad`'s superuser redirect
// (`/settings/` → `/settings/secrets`) is the route table's `superuser` guard (tooling/routes/routes.json, run on the
// server before this renders); `Suspense` around `useSearchParams` (kept from the Next-era port, harmless in the SPA;
// here inside SettingsLayout too); the four routes share this module; Prettier formatting.
// Split from SettingsRoute.tsx (stabilization phase): one module per route, so a route's page file loads only its own
// page and not every page the module served (the shared module put all of them in each route's first load).
"use client";

import { Suspense } from "react";
import { SecretsPage } from "./SecretsPage";
import { SettingsLayout } from "./SettingsLayout";

/** `/settings/secrets`: every signed-in user's own secrets. */
export function SecretsRoute() {
  return (
    <Suspense fallback={null}>
      <SettingsLayout>
        <SecretsPage />
      </SettingsLayout>
    </Suspense>
  );
}
