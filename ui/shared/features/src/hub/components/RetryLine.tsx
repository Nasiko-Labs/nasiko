/** A quiet failure line with Retry: what couldn't load, and the one action that reads it again (the hub's sections). */
import { Button } from '@nasiko/ui/primitives/button'
import { copy } from '../copy'

export function RetryLine({ label, onRetry }: { label: string; onRetry: () => void }) {
  return (
    <p className="m-0 flex items-center gap-2 text-xs text-muted-foreground">
      {label}
      <Button
        variant="link"
        size="sm"
        className="h-auto p-0 text-xs pointer-coarse:min-h-11"
        onClick={onRetry}
      >
        {copy.states.retry}
      </Button>
    </p>
  )
}
