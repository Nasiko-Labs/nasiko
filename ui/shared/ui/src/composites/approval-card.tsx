"use client";

import { CheckIcon, ClockIcon, ShieldAlertIcon, XIcon } from "lucide-react";
import { useId, useState } from "react";
import { cn } from "../lib/cn";
import type { ApprovalDecision, ApprovalRequest } from "@nasiko/contracts";
import { Badge } from "../primitives/badge";
import { Button } from "../primitives/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "../primitives/card";
import { Textarea } from "../primitives/textarea";
import { SafeLink } from "./safe-link";

/**
 * ApprovalCard presents a server-issued action request (contract §7). Every
 * state on it — pending, approved, rejected, expired, executing, failed — is
 * the server's; the card renders it and offers exactly two actions. It cannot
 * authorise anything: `onDecide` sends an `ApprovalDecision` and the server
 * decides. Animation never delays rejection or gates execution (§8.3): state
 * changes are instant, and the only motion is the primitives' own feedback.
 *
 * Accessibility: a labelled region; parameters are a definition list; secret
 * values are shown as the server redacted them; action buttons name the
 * request; the reason field is optional and labelled.
 */
export interface ApprovalCardProps {
  readonly request: ApprovalRequest;
  /** The current user's id; the controls are enabled only when they may decide. */
  readonly currentUserId: string;
  readonly onDecide?: (d: ApprovalDecision) => void | Promise<void>;
  readonly className?: string;
  readonly busy?: boolean;
}

const STATE_LABEL: Record<ApprovalRequest["state"], string> = {
  pending: "Awaiting your decision",
  approved: "Approved",
  rejected: "Rejected",
  expired: "Expired",
  executing: "Executing",
  failed: "Failed",
};

export function ApprovalCard({
  request,
  currentUserId,
  onDecide,
  className,
  busy = false,
}: ApprovalCardProps) {
  const id = useId();
  const [reason, setReason] = useState("");
  const mayDecide =
    request.state === "pending" &&
    (!request.decidableBy || request.decidableBy.includes(currentUserId)) &&
    !!onDecide;
  const decide = (decision: ApprovalDecision["decision"]) => {
    void onDecide?.({
      id: request.id,
      actionId: request.actionId,
      decision,
      ...(reason.trim() ? { reason: reason.trim() } : {}),
    });
  };
  const riskVariant =
    request.risk === "high" ? "destructive" : request.risk === "medium" ? "outline" : "secondary";
  return (
    <Card
      role="region"
      aria-labelledby={`${id}-title`}
      aria-describedby={`${id}-state`}
      data-approval-state={request.state}
      className={cn("max-w-xl", className)}
    >
      <CardHeader>
        <CardTitle id={`${id}-title`}>{request.title}</CardTitle>
        <CardDescription>
          <span className="font-mono text-xs">{request.tool}</span>
        </CardDescription>
        <CardAction className="flex items-center gap-1">
          <Badge variant={riskVariant}>
            {request.risk === "high" ? <ShieldAlertIcon aria-hidden /> : null}
            {request.risk} risk
          </Badge>
        </CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <p className="text-sm">{request.summary}</p>
        {request.params.length > 0 ? (
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
            {request.params.map((p) => (
              <div key={p.name} className="contents">
                <dt className="text-muted-foreground">{p.name}</dt>
                <dd
                  className={cn(
                    "min-w-0 break-words",
                    p.kind === "secret" && "font-mono text-muted-foreground",
                    (p.kind === "money" || p.kind === "number") && "tabular-nums",
                  )}
                >
                  {p.kind === "link" ? (
                    // The lab's Card no longer clips its box (no overflow-hidden), so a long URL must wrap inside the cell.
                    <SafeLink href={p.value} className="max-w-full [overflow-wrap:anywhere]">
                      {p.value}
                    </SafeLink>
                  ) : p.kind === "json" ? (
                    <code className="block max-h-40 overflow-auto rounded bg-muted px-2 py-1 font-mono text-xs whitespace-pre-wrap">
                      {p.value}
                    </code>
                  ) : (
                    <>
                      {p.value}
                      {p.unit ? <span className="ml-1 text-muted-foreground">{p.unit}</span> : null}
                    </>
                  )}
                </dd>
              </div>
            ))}
          </dl>
        ) : null}
        {request.links?.length ? (
          <ul className="flex flex-wrap gap-3 text-xs">
            {request.links.map((l) => (
              <li key={`${l.href}:${l.label}`}>
                <SafeLink href={l.href}>{l.label}</SafeLink>
              </li>
            ))}
          </ul>
        ) : null}
        <p
          id={`${id}-state`}
          className="flex items-center gap-1 text-xs text-muted-foreground"
          aria-live="polite"
        >
          {request.state === "pending" ? <ClockIcon className="size-3.5" aria-hidden /> : null}
          {STATE_LABEL[request.state]}
          {request.expiresAt && request.state === "pending" ? (
            <>
              {" · expires "}
              <time dateTime={request.expiresAt}>{request.expiresAt}</time>
            </>
          ) : null}
        </p>
        {mayDecide ? (
          <div className="flex flex-col gap-1">
            <label htmlFor={`${id}-reason`} className="text-xs text-muted-foreground">
              Reason (optional)
            </label>
            <Textarea
              id={`${id}-reason`}
              value={reason}
              maxLength={500}
              rows={2}
              onChange={(e) => {
                setReason(e.target.value);
              }}
            />
          </div>
        ) : null}
      </CardContent>
      {request.state === "pending" ? (
        <CardFooter className="justify-end gap-2">
          <Button
            variant="outline"
            size="sm"
            disabled={!mayDecide || busy}
            aria-label={`Reject: ${request.title}`}
            onClick={() => {
              decide("reject");
            }}
          >
            <XIcon aria-hidden />
            Reject
          </Button>
          <Button
            size="sm"
            disabled={!mayDecide || busy}
            aria-label={`Approve: ${request.title}`}
            onClick={() => {
              decide("approve");
            }}
          >
            <CheckIcon aria-hidden />
            Approve
          </Button>
        </CardFooter>
      ) : null}
    </Card>
  );
}
