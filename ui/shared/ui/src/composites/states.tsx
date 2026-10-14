"use client";

import { AlertTriangleIcon, InboxIcon, LockIcon, RefreshCwIcon } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "../lib/cn";
import { Button } from "../primitives/button";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "../primitives/empty";
import { Skeleton } from "../primitives/skeleton";
import { useMotion } from "../motion/provider";

/**
 * Feedback states every data composite shares (contract §8 "Feedback" group,
 * §8.3 loading/skeleton/empty/retry rows). Geometry is stable: a loading
 * block reserves the height of what it replaces, so content arriving never
 * shifts layout. Shimmer runs only in `full` mode and stops when loading ends.
 */

export interface LoadingStateProps {
  readonly label: string;
  /** Reserve this height (px) so completion does not shift layout. */
  readonly height?: number;
  readonly lines?: number;
  readonly className?: string;
}

export function LoadingState({ label, height, lines = 3, className }: LoadingStateProps) {
  const { mode } = useMotion();
  return (
    <div
      role="status"
      aria-busy="true"
      aria-label={label}
      className={cn("flex flex-col gap-2", className)}
      style={height ? { minHeight: height } : undefined}
      data-shimmer={mode === "full" ? "on" : "off"}
    >
      {Array.from({ length: lines }, (_, i) => (
        <Skeleton
          key={i}
          className={cn(
            "h-4",
            i === lines - 1 ? "w-2/3" : "w-full",
            mode !== "full" && "animate-none",
          )}
        />
      ))}
      <span className="sr-only">{label}</span>
    </div>
  );
}

export interface EmptyStateProps {
  readonly title: string;
  readonly description?: string;
  /** The one next action; empty states guide, they do not decorate. */
  readonly action?: ReactNode;
  readonly className?: string;
  readonly icon?: ReactNode;
}

export function EmptyState({ title, description, action, className, icon }: EmptyStateProps) {
  return (
    <Empty className={cn("py-10", className)}>
      <EmptyHeader>
        <EmptyMedia variant="icon">{icon ?? <InboxIcon aria-hidden />}</EmptyMedia>
        <EmptyTitle>{title}</EmptyTitle>
        {description ? <EmptyDescription>{description}</EmptyDescription> : null}
      </EmptyHeader>
      {action ? <EmptyContent>{action}</EmptyContent> : null}
    </Empty>
  );
}

export interface ErrorStateProps {
  readonly title: string;
  /** Already classified and safe to show (no raw server text). */
  readonly description?: string;
  readonly onRetry?: () => void;
  readonly retrying?: boolean;
  readonly requestId?: string;
  readonly className?: string;
}

/** Immediate, readable, with the one action that can help. No animation gates it. */
export function ErrorState({
  title,
  description,
  onRetry,
  retrying = false,
  requestId,
  className,
}: ErrorStateProps) {
  return (
    <div
      role="alert"
      className={cn(
        "flex flex-col items-start gap-2 rounded-lg bg-destructive/5 p-4 text-sm ring-1 ring-destructive/20",
        className,
      )}
    >
      <div className="flex items-center gap-2 font-medium text-destructive">
        <AlertTriangleIcon className="size-4" aria-hidden />
        {title}
      </div>
      {description ? <p className="text-foreground/85">{description}</p> : null}
      <div className="flex items-center gap-3">
        {onRetry ? (
          <Button size="sm" variant="outline" onClick={onRetry} disabled={retrying}>
            <RefreshCwIcon
              aria-hidden
              className={cn(retrying && "animate-spin motion-reduce:animate-none")}
            />
            {retrying ? "Retrying…" : "Retry"}
          </Button>
        ) : null}
        {requestId ? (
          <span
            className="font-mono text-xs text-foreground/70"
            aria-label={`request id ${requestId}`}
          >
            {requestId}
          </span>
        ) : null}
      </div>
    </div>
  );
}

export function DeniedState({
  title = "You don't have access to this",
  description,
  className,
}: {
  title?: string;
  description?: string;
  className?: string;
}) {
  return (
    <Empty className={cn("py-10", className)} role="status">
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <LockIcon aria-hidden />
        </EmptyMedia>
        <EmptyTitle>{title}</EmptyTitle>
        {description ? <EmptyDescription>{description}</EmptyDescription> : null}
      </EmptyHeader>
    </Empty>
  );
}

/** Data is present but incomplete: say so explicitly, in place, without hiding what is there (§8). */
export function PartialDataNotice({
  children,
  className,
}: {
  children: ReactNode;
  className?: string;
}) {
  return (
    <p
      role="status"
      className={cn(
        "flex items-center gap-2 rounded-md bg-warning/10 px-3 py-1.5 text-xs text-warning-text",
        className,
      )}
    >
      <AlertTriangleIcon className="size-3.5" aria-hidden />
      {children}
    </p>
  );
}

/** A refetch is in flight after a filter change; the previous data stays visible and readable. */
export function StaleIndicator({
  visible,
  label = "Updating…",
  className,
}: {
  visible: boolean;
  label?: string;
  className?: string;
}) {
  return (
    <span
      role="status"
      aria-live="polite"
      className={cn(
        "text-xs text-muted-foreground",
        visible ? "opacity-100" : "opacity-0",
        className,
      )}
    >
      {visible ? label : ""}
    </span>
  );
}
