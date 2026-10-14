// Ported from nasiko-ui-lab@1b99bdda common/src/features/mcp/components/DeleteDialog.tsx. Local edits: the landing
// `navigate({ to: '/mcp', search: {} })` is `useRouter().push('/mcp', { scroll: false })` from the navigation port (`@nasiko/app-core/navigation`);
// Prettier formatting.
"use client";

import { useRouter } from "@nasiko/app-core/navigation";
import { toast } from "@nasiko/ui/toast";
import { ConfirmDialog } from "../../agents/components/dialogs";
import { useDeleteConnector } from "../api";
import { copy } from "../copy";

/** Delete a server (legacy danger zone): revokes all agent access, then back to the catalog. */
export function DeleteDialog({
  id,
  label,
  open,
  onOpenChange,
}: {
  id: string;
  label: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const router = useRouter();
  const del = useDeleteConnector(id);
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={onOpenChange}
      title={copy.deleteTitle(label)}
      body={copy.deleteBody}
      confirmLabel={copy.delete}
      destructive
      pending={del.isPending}
      error={del.error}
      onConfirm={() => {
        del.mutate(undefined, {
          onSuccess: () => {
            onOpenChange(false);
            toast.success(copy.deleted(label));
            router.push("/mcp", { scroll: false });
          },
        });
      }}
    />
  );
}
