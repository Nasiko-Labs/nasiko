// Ported from nasiko-ui-lab@1b99bdda common/src/features/mcp/components/ConnectControl.tsx. Local edits: the key dialog's `onOpenChange` ternary is an `if`
// (lint: no void expression in a ternary); optional props admit `undefined` (exactOptionalPropertyTypes); Prettier formatting.
"use client";

/**
 * Connect / Disconnect for one server or toolkit (plans/feat-mcp.md §3), on catalog cards and the server page header.
 * Connected reads "Connected" and turns into "Disconnect" on hover or focus (legacy card); disconnecting confirms.
 * Connect goes by the auth flow: none at once, api_key through the key dialog, oauth through the page's popup.
 */
import { Check, Plus, X } from "lucide-react";
import { useId, useState } from "react";
import { toast } from "@nasiko/ui/toast";
import { Button } from "@nasiko/ui/primitives";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@nasiko/ui/primitives";
import { Field, FieldError, FieldLabel } from "@nasiko/ui/primitives";
import { Input } from "@nasiko/ui/primitives";
import { ConfirmDialog } from "../../agents/components/dialogs";
import { cn } from "@nasiko/ui/cn";
import { useConnect, useDisconnect } from "../api";
import { copy, reason } from "../copy";
import type { AuthFlow, ConnectOutcome } from "../types";

export interface Connectable {
  id: string;
  label: string;
  authFlow: AuthFlow;
  authType: string | null;
  connected: boolean;
}

export function ConnectControl({
  target,
  openPopup,
  size = "sm",
  className,
}: {
  target: Connectable;
  openPopup: (url: string | undefined) => void;
  size?: "sm" | "xs" | undefined;
  className?: string | undefined;
}) {
  const connect = useConnect();
  const disconnect = useDisconnect();
  const [confirm, setConfirm] = useState(false);
  const [keyOpen, setKeyOpen] = useState(false);

  const outcome = (o: ConnectOutcome) => {
    if (o.status === "connected") toast.success(copy.connectedToast(target.label));
    else openPopup(o.oauth_url ?? o.authorization_url);
  };
  const start = () => {
    if (target.authFlow === "api_key") {
      setKeyOpen(true);
      return;
    }
    connect.mutate(
      { id: target.id },
      {
        onSuccess: outcome,
        onError: (e) => toast.error(copy.connectFailed(reason(e))),
      },
    );
  };

  if (target.connected)
    return (
      <>
        <Button
          size={size}
          variant="outline"
          className={cn("group min-w-28 pointer-coarse:min-h-11", className)}
          aria-label={copy.disconnectNamed(target.label)}
          disabled={disconnect.isPending}
          onClick={() => {
            setConfirm(true);
          }}
        >
          <span className="inline-flex items-center gap-1 group-hover:hidden group-focus-visible:hidden">
            <Check className="size-3.5 text-success" aria-hidden /> {copy.connected}
          </span>
          <span className="hidden items-center gap-1 text-destructive group-hover:inline-flex group-focus-visible:inline-flex">
            <X className="size-3.5" aria-hidden /> {copy.disconnect}
          </span>
        </Button>
        <ConfirmDialog
          open={confirm}
          onOpenChange={setConfirm}
          title={copy.disconnectNamed(target.label)}
          body={copy.disconnectBody}
          confirmLabel={copy.disconnect}
          destructive
          pending={disconnect.isPending}
          error={disconnect.error}
          onConfirm={() => {
            disconnect.mutate(target.id, {
              onSuccess: () => {
                setConfirm(false);
                toast.success(copy.disconnectedToast(target.label));
              },
            });
          }}
        />
      </>
    );

  return (
    <>
      <Button
        size={size}
        variant="outline"
        className={cn("pointer-coarse:min-h-11", className)}
        aria-label={copy.connectNamed(target.label)}
        disabled={connect.isPending}
        onClick={start}
      >
        <Plus aria-hidden /> {copy.connect}
      </Button>
      {keyOpen ? (
        <KeyDialog
          target={target}
          onClose={() => {
            setKeyOpen(false);
          }}
          onConnected={(o) => {
            setKeyOpen(false);
            outcome(o);
          }}
        />
      ) : null}
    </>
  );
}

/** The API-key step (legacy connect modal): basic auth takes `username:password`. Mounted only while open. */
function KeyDialog({
  target,
  onClose,
  onConnected,
}: {
  target: Connectable;
  onClose: () => void;
  onConnected: (o: ConnectOutcome) => void;
}) {
  const id = useId();
  const connect = useConnect();
  const [value, setValue] = useState("");
  const [empty, setEmpty] = useState(false);
  const placeholder = target.authType === "basic" ? copy.basicPlaceholder : copy.keyLabel;
  return (
    <Dialog
      open
      onOpenChange={(o) => {
        if (!o && !connect.isPending) onClose();
      }}
    >
      <DialogContent>
        <form
          noValidate
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (!value.trim()) {
              setEmpty(true);
              return;
            }
            connect.mutate({ id: target.id, value: value.trim() }, { onSuccess: onConnected });
          }}
        >
          <DialogHeader>
            <DialogTitle>{copy.connectNamed(target.label)}</DialogTitle>
            <DialogDescription className="sr-only">{copy.keyLabel}</DialogDescription>
          </DialogHeader>
          <Field data-invalid={empty || connect.isError} className="gap-1.5">
            <FieldLabel htmlFor={id}>{copy.keyLabel}</FieldLabel>
            <Input
              id={id}
              type="password"
              autoComplete="off"
              placeholder={placeholder}
              value={value}
              aria-invalid={empty || connect.isError}
              onChange={(e) => {
                setValue(e.target.value);
                setEmpty(false);
              }}
            />
            {empty ? <FieldError>{copy.required}</FieldError> : null}
            {connect.isError ? (
              <FieldError>{copy.connectFailed(reason(connect.error))}</FieldError>
            ) : null}
          </Field>
          <DialogFooter>
            <Button type="button" variant="outline" disabled={connect.isPending} onClick={onClose}>
              {copy.cancel}
            </Button>
            <Button type="submit" disabled={connect.isPending}>
              {copy.connect}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
