// Ported from nasiko-ui-lab@1b99bdda common/src/components/shared/leave-guard.tsx. The lab's `useBlocker` is the
// navigation port's blocker (`LeaveBlockerContext`): in the app that is TanStack Router's `useBlocker`, as in the lab
// (router-provider.tsx), which holds in-app links, programmatic navigation (`router.push`/`replace`, search-only moves
// included), browser Back/Forward (the router's history reverts the traversal and asks first) and reload/close. Local
// edits: the blocker comes through the port, so this module imports no router; `useLeaveBlocker` is the blocker without
// the dialog (the lab's `useBlocker({ withResolver: true })` used directly by NewWorkflowPage), so a page with its own
// leave dialog keeps it; `LeaveGuard` is built on it; primitives come from `@nasiko/ui/primitives`; Prettier
// formatting. Navigation to /login always goes through (the lab's rule): a 401 sends the page there, and an expired
// session cannot save anyway. A navigation that must pass a dirty form (the lab's `ignoreBlocker: true`) asks the
// navigation port for `{ ignoreBlocker: true }`.
import { useContext } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@nasiko/ui/primitives";
import { LeaveBlockerContext, type LeaveBlocker } from "./navigation";

export type { LeaveBlocker } from "./navigation";

/**
 * The guard without its dialog (the lab's `useBlocker({ withResolver: true })`), for a page whose leave dialog has
 * its own choices (Workflows' new page: Keep editing, Discard, Save draft). Same rules as `LeaveGuard`.
 */
export function useLeaveBlocker({
  when,
  samePath = false,
}: {
  when: boolean;
  /** Let search-only moves through (a form that spans the page's `?section=` views keeps its edits). */
  samePath?: boolean | undefined;
}): LeaveBlocker {
  const useBlocker = useContext(LeaveBlockerContext);
  if (!useBlocker)
    throw new Error("LeaveGuard needs the navigation port's blocker (RouterNavigation)");
  return useBlocker({ when, samePath });
}

/**
 * Asks before leaving a route with unsaved form edits (plan §2.4): router navigation gets an `AlertDialog`, a reload
 * or tab close gets the browser's own prompt. Render it inside the form's component with
 * `when={formState.isDirty && !formState.isSubmitting}`.
 */
export function LeaveGuard({
  when,
  title = "Leave without saving?",
  description = "Your changes on this page will be lost.",
  samePath = false,
}: {
  when: boolean;
  title?: string;
  description?: string;
  /** Let search-only moves through (a form that spans the page's `?section=` views keeps its edits). */
  samePath?: boolean;
}) {
  const blocker = useLeaveBlocker({ when, samePath });
  return (
    <AlertDialog
      open={blocker.blocked}
      onOpenChange={(open) => {
        if (!open) blocker.reset();
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel onClick={blocker.reset}>Stay</AlertDialogCancel>
          <AlertDialogAction onClick={blocker.proceed}>Leave</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
