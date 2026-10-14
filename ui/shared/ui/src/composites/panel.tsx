// Ported from nasiko-ui-lab@1b99bdda common/src/components/shared/panel.tsx. Local edits:
// - `Card asChild` -> `Card render={<section …/>}` (Base UI useRender; same `<section>` element and attributes);
// - the lab's `ApiError` (`@/lib/api/client`, not in this package) is read structurally: `readApiError` accepts any
//   error with a numeric `status` plus the optional `path`, `serverMessage` and `isForbidden` (403 when absent) fields
//   the lab's class exposes, so the same copy renders for any client's error class;
// - imports mapped to ../primitives/* and ../lib/cn.
"use client";

/**
 * Panel chrome and in-panel states on shadcn `Card`, `Alert`, `Empty` and `Skeleton` (TokenOps plan
 * A2b, shared since plan §8 Phase 1): every panel owns its skeleton, error card (with the request path
 * and Retry) and empty state, so one failed query never blanks the page.
 */
import { AlertTriangle, Lock, RotateCw } from "lucide-react";
import type { ReactNode, Ref } from "react";
import { Alert, AlertDescription, AlertTitle } from "../primitives/alert";
import { Button } from "../primitives/button";
import { Card, CardAction, CardDescription, CardHeader } from "../primitives/card";
import { Skeleton } from "../primitives/skeleton";
import { cn } from "../lib/cn";
import { EmptyState } from "./state-card";

export function Panel({
  title,
  subtitle,
  actions,
  children,
  labelledBy,
  focusableTitle,
  className,
  ref,
  titleRef,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  /** The heading's id; names the section. */
  labelledBy?: string | undefined;
  /** Lets code move focus to the heading (tabIndex -1) when the panel opens. */
  focusableTitle?: boolean | undefined;
  className?: string | undefined;
  ref?: Ref<HTMLElement> | undefined;
  titleRef?: Ref<HTMLHeadingElement> | undefined;
}) {
  return (
    <Card
      className={cn("min-w-0 gap-3 p-4", className)}
      render={<section ref={ref} aria-labelledby={labelledBy} />}
    >
      <CardHeader className="flex flex-wrap items-start justify-between gap-2 px-0">
        {/* A small basis, so a long subtitle wraps beside the actions instead of pushing them to their own row. */}
        <div className="min-w-0 flex-[1_1_15rem]">
          <h2
            ref={titleRef}
            id={labelledBy}
            tabIndex={focusableTitle ? -1 : undefined}
            className="text-sm font-semibold outline-none"
          >
            {title}
          </h2>
          {subtitle ? <CardDescription className="text-xs">{subtitle}</CardDescription> : null}
        </div>
        {actions ? (
          <CardAction className="col-auto row-auto flex flex-wrap items-center gap-2">
            {actions}
          </CardAction>
        ) : null}
      </CardHeader>
      {children}
    </Card>
  );
}

export function PanelSkeleton({ height = 180 }: { height?: number | undefined }) {
  return (
    // Decorative: not a live region (a page of panels would announce "Loading" once per panel). The page marks its
    // region aria-busy while it loads (plan §8 Phase 9).
    <Skeleton aria-hidden className="w-full rounded-md" style={{ height }} />
  );
}

/** The fields `PanelError` reads from an API error (the lab's `ApiError` shape), read structurally from any error class. */
export interface PanelApiError {
  status: number;
  path?: string | undefined;
  serverMessage?: string | null | undefined;
  isForbidden?: boolean | undefined;
}

export function readApiError(error: unknown): PanelApiError | null {
  if (!error || typeof error !== "object") return null;
  const e = error as Record<string, unknown>;
  if (typeof e.status !== "number") return null;
  return {
    status: e.status,
    path: typeof e.path === "string" ? e.path : undefined,
    serverMessage: typeof e.serverMessage === "string" ? e.serverMessage : null,
    // A 403 is never session loss (only a 401 is): render an inline "No access" state.
    isForbidden: typeof e.isForbidden === "boolean" ? e.isForbidden : e.status === 403,
  };
}

export function PanelError({
  error,
  onRetry,
  what,
}: {
  error: unknown;
  onRetry: () => void;
  what: string;
}) {
  const api = readApiError(error);
  // A 403 is an answer, not a failure: nothing to retry until someone grants access.
  if (api?.isForbidden) {
    return (
      <Alert className="gap-y-2 [&>svg]:text-warning">
        <Lock aria-hidden />
        <AlertTitle className="line-clamp-none">No access to {what}</AlertTitle>
        <AlertDescription className="text-xs">
          {api.serverMessage ? `The server said: ${api.serverMessage}. ` : null}Ask an admin or the
          owner for access.
        </AlertDescription>
      </Alert>
    );
  }
  return (
    <Alert className="gap-y-2 border-destructive/30 bg-destructive/5 [&>svg]:text-destructive">
      <AlertTriangle aria-hidden />
      <AlertTitle className="line-clamp-none text-destructive">Couldn't load {what}</AlertTitle>
      <AlertDescription className="gap-2">
        <p className="text-xs">
          {api?.serverMessage ?? (api ? `Request failed (${api.status}).` : "Unexpected error.")}
          {api?.path !== undefined ? (
            <>
              {" "}
              <code className="break-all">{api.path}</code>
            </>
          ) : null}
        </p>
        <Button size="sm" variant="outline" onClick={onRetry}>
          <RotateCw className="size-3.5" aria-hidden /> Retry
        </Button>
      </AlertDescription>
    </Alert>
  );
}

export function PanelEmpty({ title, children }: { title: string; children?: ReactNode }) {
  return <EmptyState title={title}>{children}</EmptyState>;
}
