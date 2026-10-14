// Ported from nasiko-ui-lab@1b99bdda common/src/features/workflows/run.ts. Local edits: `"use client"`; TanStack's
// `useNavigate` -> `useRouter().push` from the navigation port (`@nasiko/app-core/navigation`); the success's "only while the user is still on the Runs
// tab" (the lab read the global `router.state.location.pathname` and navigated there with `search: (s) => ({ ...s,
// run })`, replacing) is a module-level landing the mounted Runs page registers (`useRunsTabLanding`, new in the Next-era port,
// whose navigation port has no global router state a mutation callback can read after the clicking page unmounts), which merges `run` into
// that page's own search with a replace; the mutation key and invalidations are scoped; `onError` is braced
// (no-confusing-void-expression); Prettier formatting.
"use client";

/**
 * Run opens the Runs tab at once (user request 2026-10-02): it shows a starting card until the 202 names the run, then
 * opens that run (`?run=`). Back pops straight to where Run was clicked.
 */
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@nasiko/app-core/navigation";
import { useEffect, useLayoutEffect, useRef } from "react";
import { useProductScope } from "@nasiko/data-access/product";
import { toast } from "@nasiko/ui/toast";
import { runWorkflow, workflowKeys } from "./api";
import { copy, reason } from "./copy";

/** The Runs page on screen: opens a started run in place (`?run=`, replacing). Null when the page isn't mounted. */
let runsTab: ((run: string) => void) | null = null;

/** Mounted by the Runs page: a run started from anywhere opens there while it is on screen. */
export function useRunsTabLanding(open: (run: string) => void) {
  const latest = useRef(open);
  useLayoutEffect(() => {
    latest.current = open;
  });
  useEffect(() => {
    const land = (run: string) => {
      latest.current(run);
    };
    runsTab = land;
    return () => {
      if (runsTab === land) runsTab = null;
    };
  }, []);
}

export function useStartRun() {
  const router = useRouter();
  const queryClient = useQueryClient();
  const scope = useProductScope();
  // Hook-level callbacks: they still fire after the page that clicked Run has unmounted.
  const start = useMutation({
    mutationKey: workflowKeys.start(scope),
    mutationFn: (id: string) => runWorkflow(id),
    onSuccess: (r) => {
      // Only while the user is still on the Runs tab: never pull them back from elsewhere.
      runsTab?.(r.execution_id);
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: workflowKeys.runs(scope) });
      void queryClient.invalidateQueries({ queryKey: workflowKeys.lists(scope) });
    },
    onError: (err) => {
      toast.error(copy.runFailed(reason(err)));
    },
  });
  return (id: string) => {
    start.mutate(id);
    router.push("/workflows/runs", { scroll: false });
  };
}
