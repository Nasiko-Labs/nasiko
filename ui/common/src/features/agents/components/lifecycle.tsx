/**
 * Restart / Stop / Start with the truth check (plan §6.2), shared by the Your agents rows and
 * the detail header. Actions come from the status matrix only. Restart and Start begin a watch
 * that reports the real outcome ("Restarted" only after Running holds); Stop confirms first.
 */
import { Button } from '@/components/ui/button'
import { copy } from '../copy'
import { cn } from '@/lib/utils'
import { ACTION_LABEL, outcomeText, type LifecycleFlow } from '../lifecycleFlow'
import { actionsFor, type DisplayStatus } from '../status'
import { ErrorNote } from './bits'
import { ConfirmDialog } from './dialogs'

/** `collapse`: below 640 px only the primary action shows; the caller offers the rest in its overflow (§7.3). */
export function LifecycleButtons({
  flow,
  display,
  name,
  size = 'sm',
  collapse = false,
}: {
  flow: LifecycleFlow
  display: DisplayStatus
  name: string
  size?: 'sm' | 'default'
  collapse?: boolean
}) {
  const actions = actionsFor(display)
  const note = outcomeText(flow)
  if (!actions.length && !note) return null
  return (
    <div className="flex flex-col items-end gap-1">
      <div className="flex flex-wrap gap-2">
        {actions.map((a, i) => (
          <Button
            key={a}
            size={size}
            variant={i === 0 && a !== 'stop' ? 'default' : 'outline'}
            className={cn(collapse && i > 0 && 'hidden sm:inline-flex')}
            disabled={flow.m.isPending || flow.w.watching}
            onClick={() => flow.run(a)}
          >
            {ACTION_LABEL[a]}
          </Button>
        ))}
      </div>
      {note ? (
        <span
          role="status"
          className={
            note.tone === 'warning' ? 'text-xs text-warning' : 'text-xs text-muted-foreground'
          }
        >
          {note.text}
        </span>
      ) : null}
      {flow.m.isError && !flow.stopOpen ? (
        <ErrorNote error={flow.m.error} context="lifecycle" className="text-xs" />
      ) : null}
      <StopDialog flow={flow} name={name} />
    </div>
  )
}

export function StopDialog({ flow, name }: { flow: LifecycleFlow; name: string }) {
  return (
    <ConfirmDialog
      open={flow.stopOpen}
      onOpenChange={flow.setStopOpen}
      title={copy.stopTitle(name)}
      body={copy.stopBody}
      confirmLabel={copy.stop}
      destructive
      pending={flow.m.isPending}
      error={flow.m.isError ? flow.m.error : undefined}
      errorContext="lifecycle"
      onConfirm={flow.confirmStop}
    />
  )
}
