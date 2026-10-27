/**
 * Run opens the Runs tab at once (user request 2026-10-02): it shows a starting card until the 202 names the run, then
 * opens that run (`?run=`). Back pops straight to where Run was clicked.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useNavigate, useRouter } from '@tanstack/react-router'
import { toast } from 'sonner'
import { runWorkflow, workflowKeys } from './api'
import { copy, reason } from './copy'

export function useStartRun() {
  const navigate = useNavigate()
  const router = useRouter()
  const queryClient = useQueryClient()
  // Hook-level callbacks: they still fire after the page that clicked Run has unmounted.
  const start = useMutation({
    mutationKey: workflowKeys.start,
    mutationFn: (id: string) => runWorkflow(id),
    onSuccess: (r) => {
      // Only while the user is still on the Runs tab: never pull them back from elsewhere.
      if (router.state.location.pathname === '/workflows/runs')
        void navigate({
          to: '/workflows/runs',
          search: (s) => ({ ...s, run: r.execution_id }),
          replace: true,
        })
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: workflowKeys.runs })
      void queryClient.invalidateQueries({ queryKey: workflowKeys.lists })
    },
    onError: (err) => toast.error(copy.runFailed(reason(err))),
  })
  return (id: string) => {
    start.mutate(id)
    void navigate({ to: '/workflows/runs', search: {} })
  }
}
