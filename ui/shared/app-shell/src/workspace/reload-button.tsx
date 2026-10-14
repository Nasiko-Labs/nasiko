// Ported from nasiko-ui-lab@1b99bdda common/src/app/shell/RouteStates.tsx (the Try again button). Local edits: a
// client component whose retry is `router.refresh()` (the server layout re-runs the identity check), since the
// failure came from a server render and no error boundary `reset` exists for it; Prettier formatting.
"use client";

import { useRouter } from "@nasiko/app-core/navigation";
import { Button } from "@nasiko/ui/primitives";
import { copy } from "@nasiko/app-core/shell/copy";

export function ReloadButton() {
  const router = useRouter();
  return (
    <Button
      size="sm"
      variant="outline"
      onClick={() => {
        router.refresh();
      }}
    >
      {copy.routeError.tryAgain}
    </Button>
  );
}
