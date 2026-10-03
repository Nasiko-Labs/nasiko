import { useMutation } from '@tanstack/react-query'
import { Loader2 } from 'lucide-react'
import { toast } from 'sonner'
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { deleteWorkflow } from '../api'
import { copy, reason } from '../copy'
import type { Workflow } from '../types'

/** Delete behind a confirm (a soft delete: the runs are kept). Open while `workflow` is set. */
export function DeleteWorkflowDialog({
  workflow,
  onClose,
  onDeleted,
}: {
  workflow: Workflow | null
  onClose: () => void
  onDeleted: (wf: Workflow) => void
}) {
  const remove = useMutation({
    mutationFn: (wf: Workflow) => deleteWorkflow(wf.id),
    onSuccess: (_, wf) => {
      onClose()
      onDeleted(wf)
    },
    onError: (err) => toast.error(copy.deleteFailed(reason(err))),
  })
  return (
    <AlertDialog open={!!workflow} onOpenChange={(o) => !o && !remove.isPending && onClose()}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{copy.deleteTitle(workflow?.name ?? '')}</AlertDialogTitle>
          <AlertDialogDescription>{copy.deleteText(workflow?.name ?? '')}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={remove.isPending}>{copy.cancel}</AlertDialogCancel>
          <Button
            variant="destructive"
            disabled={remove.isPending}
            onClick={() => workflow && remove.mutate(workflow)}
          >
            {remove.isPending ? <Loader2 className="animate-spin" aria-hidden /> : null}
            {copy.confirmDelete}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
