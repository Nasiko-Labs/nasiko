// Destination file (no lab counterpart). The notice for a tab that has found its build is stale (a chunk it tried to
// preload is gone from the server): it never reloads by itself, because the user may be in the middle of an edit
// (@nasiko/app-core/stale-build). A manual reload with unsaved edits still meets the leave guard's browser prompt.
import { useStaleBuild } from "@nasiko/app-core/stale-build";
import { Button } from "@nasiko/ui/primitives";

export function StaleBuildNotice() {
  const stale = useStaleBuild();
  if (!stale) return null;
  return (
    <div
      role="status"
      data-testid="stale-build-notice"
      className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border bg-muted px-4 py-2 text-sm"
    >
      <span>A new version of Nasiko is available. Save any changes, then reload.</span>
      <Button
        size="sm"
        variant="outline"
        onClick={() => {
          window.location.reload();
        }}
      >
        Reload
      </Button>
    </div>
  );
}
