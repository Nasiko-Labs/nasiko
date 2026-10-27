/**
 * The notice for a tab that has found its build is stale (a chunk it tried to preload is gone from the server). It
 * never reloads by itself, because the user may be in the middle of an edit (lib/staleBuild.ts); a manual reload with
 * unsaved edits still meets the leave guard's browser prompt.
 */
import { Button } from '@nasiko/ui/primitives/button'
import { useStaleBuild } from '@nasiko/app-core/staleBuild'
import { copy } from '@nasiko/app-core/shell/copy'

export function StaleBuildNotice() {
  const stale = useStaleBuild()
  if (!stale) return null
  return (
    <div
      role="status"
      className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border bg-muted px-4 py-2 text-sm"
    >
      <span>{copy.staleBuild.notice}</span>
      <Button
        size="sm"
        variant="outline"
        onClick={() => {
          window.location.reload()
        }}
      >
        {copy.staleBuild.reload}
      </Button>
    </div>
  )
}
