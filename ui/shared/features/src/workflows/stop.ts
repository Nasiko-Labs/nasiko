/**
 * Stopping runs (nasiko-cloud-rs `feat/maf-stop-execution`: `POST /maf/execution/{id}/stop` and
 * `POST /maf/executions/stop`). The server decides what stops: a run that finished between the click and the stop is a
 * 409, which is news ("it had already finished"), not a failure. Either way the runs and the list metrics are re-read,
 * so the badge shows what the server now says.
 */
import { useMutation, useQueryClient, type QueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { ApiError } from '@nasiko/data-access/client'
import { stopExecution, stopWorkflowRuns, workflowKeys } from './api'
import { copy, reason } from './copy'

function refresh(queryClient: QueryClient) {
  // Every run read (one run and the runs list), and the lists whose success rate leaves stopped runs out.
  void queryClient.invalidateQueries({ queryKey: workflowKeys.runs })
  void queryClient.invalidateQueries({ queryKey: workflowKeys.lists })
}

/** A 409 from a stop: the run was already over. */
const alreadyFinished = (err: unknown) => err instanceof ApiError && err.status === 409

export function useStopRun() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => stopExecution(id),
    onSuccess: () => toast.success(copy.runStopped),
    onError: (err) =>
      alreadyFinished(err)
        ? toast(copy.runAlreadyFinished)
        : toast.error(copy.stopFailed(reason(err))),
    onSettled: () => refresh(queryClient),
  })
}

export function useStopWorkflowRuns() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (workflowId: string) => stopWorkflowRuns(workflowId),
    onSuccess: (report) => toast.success(copy.runsStopped(report.stopped.length)),
    onError: (err) => toast.error(copy.stopFailed(reason(err))),
    onSettled: () => refresh(queryClient),
  })
}
