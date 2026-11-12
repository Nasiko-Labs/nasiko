/**
 * Restart / Stop / Start with the truth check (plan §6.2), shared by the Your agents rows and
 * the detail header. Actions come from the status matrix only. Restart and Start begin a watch
 * that reports the real outcome ("Restarted" only after Running holds); Stop confirms first.
 */
import { Button } from '@nasiko/ui/primitives/button'
import { copy } from '../copy'
import { cn } from '@nasiko/ui/lib/utils'
import { useGrants } from '../api'
import { isSharedWithOthers } from '../grants'
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
    // The note sits beside the buttons, not under them: a second line would lift them out of the top bar's row.
    <div className="flex flex-wrap items-center justify-end gap-2">
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
      {actions.map((a, i) => (
        <Button
          key={a}
          size={size}
          // A healthy agent's lifecycle actions are maintenance, not the page's primary — Try it is (a parked one's too).
          variant={
            i === 0 && a !== 'stop' && display !== 'running' && display !== 'parked'
              ? 'default'
              : 'outline'
          }
          className={cn(collapse && i > 0 && 'hidden sm:inline-flex')}
          disabled={flow.m.isPending || flow.w.watching}
          onClick={() => flow.run(a)}
        >
          {ACTION_LABEL[a]}
        </Button>
      ))}
      {flow.m.isError && !flow.stopOpen ? (
        <ErrorNote
          error={flow.m.error}
          context="lifecycle"
          className="basis-full text-right text-xs"
        />
      ) : null}
      <StopDialog flow={flow} name={name} />
    </div>
  )
}

export function StopDialog({ flow, name }: { flow: LifecycleFlow; name: string }) {
  // Read only once the dialog opens; a failed read just leaves the warning out.
  const grants = useGrants(flow.id, flow.stopOpen)
  const shared = !!grants.data && isSharedWithOthers(grants.data)
  return (
    <ConfirmDialog
      open={flow.stopOpen}
      onOpenChange={flow.setStopOpen}
      title={copy.stopTitle(name)}
      body={
        shared ? (
          <>
            <span className="block">{copy.stopBody}</span>
            <span role="note" className="mt-2 block text-warning">
              {copy.stopSharedWarning}
            </span>
          </>
        ) : (
          copy.stopBody
        )
      }
      confirmLabel={copy.stop}
      destructive
      pending={flow.m.isPending}
      error={flow.m.isError ? flow.m.error : undefined}
      errorContext="lifecycle"
      onConfirm={flow.confirmStop}
    />
  )
}
