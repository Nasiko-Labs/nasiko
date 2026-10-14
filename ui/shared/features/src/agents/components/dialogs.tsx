// Ported from nasiko-ui-lab@1b99bdda common/src/features/agents/components/dialogs.tsx. Local edits: `meQuery` is
// `meQueryOptions(useProductScope())`; the delete's landing is `useRouter().replace(hrefFor('/agents/mine', { owner
// }))` from the navigation port (`@nasiko/app-core/navigation`), and the result the lab put in TanStack history state (`state.agentDeleted`) is handed
// to that page with `carryDeletedNote` (../deletedNote.ts), which stamps it into the entry's history state on
// arrival; optional props admit `undefined` (exactOptionalPropertyTypes); Prettier formatting.
"use client";

/**
 * Confirm dialogs for destructive actions (plan §6.2). Radix restores focus to the trigger on
 * close. Actions stay disabled while their mutation is pending (no double submit).
 */
import { useQuery } from "@tanstack/react-query";
import { useRouter } from "@nasiko/app-core/navigation";
import { useId, useState, type ReactNode } from "react";
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
import { Input } from "@nasiko/ui/primitives";
import { Textarea } from "@nasiko/ui/primitives";
import { meQueryOptions, useProductScope } from "@nasiko/data-access/product";
import { useDeleteAgent } from "../api";
import { carryDeletedNote } from "../deletedNote";
import { hrefFor } from "@nasiko/app-core/url-state";
import { copy, type ErrorContext } from "../copy";
import type { AgentView } from "../normalize";
import { ErrorNote } from "./bits";

interface ConfirmProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  body: ReactNode;
  confirmLabel: string;
  destructive?: boolean | undefined;
  pending?: boolean | undefined;
  error?: unknown;
  errorContext?: ErrorContext | undefined;
  onConfirm: () => void;
  children?: ReactNode | undefined;
  confirmDisabled?: boolean | undefined;
}

export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  body,
  confirmLabel,
  destructive,
  pending,
  error,
  errorContext,
  onConfirm,
  children,
  confirmDisabled,
}: ConfirmProps) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{body}</AlertDialogDescription>
        </AlertDialogHeader>
        {children}
        {error ? <ErrorNote error={error} context={errorContext} /> : null}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>{copy.cancel}</AlertDialogCancel>
          {/* A plain Button, not AlertDialogAction: the dialog stays open until the call settles. */}
          <Button
            variant={destructive ? "destructive" : "default"}
            disabled={pending || confirmDisabled}
            onClick={onConfirm}
          >
            {pending ? `${confirmLabel}…` : confirmLabel}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

/** Delete: type the agent's unique `name` (not the display name) to enable the button. */
function DeleteDialog({
  open,
  onOpenChange,
  name,
  pending,
  error,
  onConfirm,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  name: string;
  pending: boolean;
  error: unknown;
  onConfirm: () => void;
}) {
  const [typed, setTyped] = useState("");
  const id = useId();
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={(o) => {
        if (!o) setTyped("");
        onOpenChange(o);
      }}
      title={copy.deleteTitle(name)}
      body={copy.deleteBody}
      confirmLabel={copy.deleteAgent}
      destructive
      pending={pending}
      error={error}
      errorContext="manage"
      confirmDisabled={typed !== name}
      onConfirm={onConfirm}
    >
      <label htmlFor={id} className="text-sm">
        {copy.deleteConfirmLead} <code className="font-mono">{name}</code> {copy.deleteConfirmTail}
      </label>
      <Input
        id={id}
        value={typed}
        onChange={(e) => {
          setTyped(e.target.value);
        }}
        autoComplete="off"
        spellCheck={false}
        className="font-mono"
      />
    </ConfirmDialog>
  );
}

export function RollbackDialog({
  open,
  onOpenChange,
  version,
  pending,
  error,
  onConfirm,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  version: string;
  pending: boolean;
  error: unknown;
  onConfirm: (reason: string) => void;
}) {
  const [reason, setReason] = useState("");
  const id = useId();
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={(o) => {
        if (!o) setReason("");
        onOpenChange(o);
      }}
      title={copy.rollBackTitle(version)}
      body={copy.rollBackBody}
      confirmLabel={copy.rollBackTo}
      pending={pending}
      error={error}
      errorContext="rollback"
      onConfirm={() => {
        onConfirm(reason.trim());
      }}
    >
      <label htmlFor={id} className="text-sm">
        {copy.rollBackReason}
      </label>
      <Textarea
        id={id}
        value={reason}
        onChange={(e) => {
          setReason(e.target.value);
        }}
        rows={2}
      />
    </ConfirmDialog>
  );
}

/**
 * Delete from anywhere on the detail page: the mutation, the type-the-name dialog and the
 * landing on Your agents with the result (replace: Back must not reopen a deleted agent).
 */
export function DeleteAgentDialog({
  agent,
  open,
  onOpenChange,
}: {
  agent: AgentView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const del = useDeleteAgent(agent.id);
  const router = useRouter();
  const me = useQuery(meQueryOptions(useProductScope())).data;
  return (
    <DeleteDialog
      open={open}
      onOpenChange={(o) => {
        if (o) del.reset();
        onOpenChange(o);
      }}
      name={agent.name}
      pending={del.isPending}
      error={del.isError ? del.error : undefined}
      onConfirm={() => {
        del.mutate(undefined, {
          onSuccess: (res) => {
            carryDeletedNote("/agents/mine", {
              name: agent.displayName,
              // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- defensive read of server data (the lab's)
              errors: res?.runtime_errors ?? [],
              // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- defensive read of server data (the lab's)
              stopped: res?.containers_stopped ?? 0,
            });
            router.replace(
              // A superuser deleting someone else's agent lands on that owner's list.
              hrefFor(
                "/agents/mine",
                agent.ownerId && agent.ownerId !== me?.sub ? { owner: agent.ownerId } : {},
              ),
            );
          },
        });
      }}
    />
  );
}
