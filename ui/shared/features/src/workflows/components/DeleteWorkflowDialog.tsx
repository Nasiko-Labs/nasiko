// Ported from nasiko-ui-lab@1b99bdda common/src/features/workflows/components/DeleteWorkflowDialog.tsx. Local edits:
// `"use client"`; callbacks that returned a void expression are braced (lint); Prettier formatting.
"use client";

import { useMutation } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import { toast } from "@nasiko/ui/toast";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@nasiko/ui/primitives";
import { Button } from "@nasiko/ui/primitives";
import { deleteWorkflow } from "../api";
import { copy, reason } from "../copy";
import type { Workflow } from "../types";

/** Delete behind a confirm (a soft delete: the runs are kept). Open while `workflow` is set. */
export function DeleteWorkflowDialog({
  workflow,
  onClose,
  onDeleted,
}: {
  workflow: Workflow | null;
  onClose: () => void;
  onDeleted: (wf: Workflow) => void;
}) {
  const remove = useMutation({
    mutationFn: (wf: Workflow) => deleteWorkflow(wf.id),
    onSuccess: (_, wf) => {
      onClose();
      onDeleted(wf);
    },
    onError: (err) => toast.error(copy.deleteFailed(reason(err))),
  });
  return (
    <AlertDialog
      open={!!workflow}
      onOpenChange={(o) => {
        if (!o && !remove.isPending) onClose();
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{copy.deleteTitle(workflow?.name ?? "")}</AlertDialogTitle>
          <AlertDialogDescription>{copy.deleteText(workflow?.name ?? "")}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={remove.isPending}>{copy.cancel}</AlertDialogCancel>
          <Button
            variant="destructive"
            disabled={remove.isPending}
            onClick={() => {
              if (workflow) remove.mutate(workflow);
            }}
          >
            {remove.isPending ? <Loader2 className="animate-spin" aria-hidden /> : null}
            {copy.confirmDelete}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
