/** Stop behind a confirm: one run (the run page, a runs-list card) or every unfinished run of a workflow. */
import { CircleStop, Loader2 } from 'lucide-react'
import { useState } from 'react'
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@nasiko/ui/primitives/alert-dialog'
import { Button } from '@nasiko/ui/primitives/button'
import { copy } from '../copy'
import { useStopRun, useStopWorkflowRuns } from '../stop'

type Target =
  /** One run: `title` names it in the confirm ("Execution #12"). */
  | { run: string; title: string }
  /** Every unfinished run of a workflow; `unfinished` is how many the page knows of. */
  | { workflow: string; name: string; unfinished: number }

export function StopRunButton({ target }: { target: Target }) {
  const [asking, setAsking] = useState(false)
  const stopRun = useStopRun()
  const stopAll = useStopWorkflowRuns()
  const pending = stopRun.isPending || stopAll.isPending
  const one = 'run' in target
  const confirm = () => {
    const done = { onSettled: () => setAsking(false) }
    if (one) stopRun.mutate(target.run, done)
    else stopAll.mutate(target.workflow, done)
  }
  return (
    <>
      <Button
        variant="outline"
        size="sm"
        className="border-destructive/40 text-destructive hover:text-destructive"
        disabled={pending}
        onClick={() => setAsking(true)}
      >
        <CircleStop aria-hidden /> {one ? copy.stopRun : copy.stopRuns(target.unfinished)}
      </Button>
      <AlertDialog open={asking} onOpenChange={(o) => !o && !pending && setAsking(false)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {one ? copy.stopRunTitle(target.title) : copy.stopRunsTitle(target.name)}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {one ? copy.stopRunText : copy.stopRunsText}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={pending}>{copy.keepRunning}</AlertDialogCancel>
            <Button variant="destructive" disabled={pending} onClick={confirm}>
              {pending ? <Loader2 className="animate-spin" aria-hidden /> : null}
              {copy.confirmStop}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}
