/**
 * Asks before leaving a route with unsaved form edits (plan §2.4): router navigation gets an
 * `AlertDialog`, a reload or tab close gets the browser's own prompt. Render it inside the form's
 * component with `when={formState.isDirty && !formState.isSubmitting}`. Navigation to /login always goes
 * through: a 401 redirects there (src/lib/queryClient.ts), and an expired session can't save anyway.
 */
import { useBlocker } from '@tanstack/react-router'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'

export function LeaveGuard({
  when,
  title = 'Leave without saving?',
  description = 'Your changes on this page will be lost.',
  samePath = false,
}: {
  when: boolean
  title?: string
  description?: string
  /** Let search-only moves through (a form that spans the page's `?section=` views keeps its edits). */
  samePath?: boolean
}) {
  const blocker = useBlocker({
    shouldBlockFn: ({ current, next }) =>
      when && next.pathname !== '/login' && !(samePath && next.pathname === current.pathname),
    enableBeforeUnload: () => when,
    withResolver: true,
  })
  return (
    <AlertDialog
      open={blocker.status === 'blocked'}
      onOpenChange={(open) => {
        if (!open) blocker.reset?.()
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel onClick={() => blocker.reset?.()}>Stay</AlertDialogCancel>
          <AlertDialogAction onClick={() => blocker.proceed?.()}>Leave</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
