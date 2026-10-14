// Ported from nasiko-ui-lab@1b99bdda common/src/features/tokenops/components/TracesDrawer.tsx. Local edits: Radix's
// `onOpenAutoFocus`/`onCloseAutoFocus` become Base UI Dialog's `initialFocus`/`finalFocus` on `SheetContent` (same
// behaviour: focus lands on the heading when opened, returns to the remembered opener on close, and stays where
// Base UI's default leaves it — the body for a deep link — when nothing opened it); `isTopTracesAbsent` is
// `isEndpointAbsent` from `@nasiko/data-access/product` (the lab's alias lives in its detect.ts); `Button asChild` around the
// trace anchor -> the anchor with `buttonVariants(...)` classes (Base UI's Button would put `role="button"` on it;
// the lab's Slot kept it a link); the mock hint names this app's `?mock=` knob; primitives from `@nasiko/ui/primitives`,
// composites from `@nasiko/ui/composites`, formatters from `@nasiko/ui/lib`; lint-driven mechanical edits (void arrow bodies braced,
// `&apos;` in JSX text); Prettier formatting.
"use client";

/**
 * F4 — "Which traces burned it?" (plan F4, A2b, A5, A11, A18).
 *
 * Built on the PROPOSED `/finops/top-traces` contract. In live mode the endpoint does
 * not exist yet: a 404 renders an explanatory state, not an error. Links open the
 * existing nasiko UI session/trace view in a new tab.
 */
import { ExternalLink, Info } from "lucide-react";
import { useRef } from "react";
import {
  buttonVariants,
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@nasiko/ui/primitives";
import { isEndpointAbsent } from "@nasiko/data-access/product";
import { TOP_TRACES_LIMIT } from "../api";
import { fmtLatency, fmtLocalDateTime, fmtMoney, fmtTokens } from "@nasiko/ui/lib";
import { traceLink } from "../links";
import type { TopTracesData } from "../types";
import { PanelEmpty, PanelError, PanelSkeleton } from "@nasiko/ui/composites";

/** The proposed `/finops/top-traces` is absent on this server (a bare 404), as opposed to an answer. */
const isTopTracesAbsent = (err: unknown) => isEndpointAbsent(err);

export function TracesDrawer({
  open,
  onOpenChange,
  scopeLabel,
  data,
  loading,
  error,
  onRetry,
  legacyUiUrl,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  scopeLabel: string;
  data: TopTracesData | undefined;
  loading: boolean;
  error: unknown;
  onRetry: () => void;
  legacyUiUrl: string | null;
}) {
  const titleRef = useRef<HTMLHeadingElement | null>(null);
  // The drawer opens from the URL, not a Radix trigger, so Radix has nowhere to return
  // focus on close. Remember what was focused when it opened and go back there.
  const openerRef = useRef<HTMLElement | null>(null);
  // A route-level 404 means this server predates the proposed endpoint. A 404 whose body is
  // "agent not found" is an access answer for the agent filter, like the dashboard's (A29).
  const missingEndpoint = isTopTracesAbsent(error);
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side="right"
        className="w-full gap-0 overflow-y-auto sm:max-w-130"
        initialFocus={() => {
          const active = document.activeElement;
          openerRef.current =
            active instanceof HTMLElement && active !== document.body ? active : null;
          // Land on the heading so screen readers announce what opened (A11).
          return titleRef.current;
        }}
        finalFocus={() => {
          const opener = openerRef.current;
          openerRef.current = null;
          if (opener?.isConnected) return opener;
          return false;
        }}
      >
        <SheetHeader>
          <SheetTitle ref={titleRef} tabIndex={-1} className="outline-none">
            Most expensive traces
          </SheetTitle>
          <SheetDescription>{scopeLabel}</SheetDescription>
        </SheetHeader>
        <div className="px-4 pb-6">
          {loading && !data ? (
            <PanelSkeleton height={300} />
          ) : missingEndpoint ? (
            <div
              role="status"
              className="flex gap-2 rounded-md border border-info/30 bg-info/5 p-3 text-sm"
            >
              <Info className="mt-0.5 size-4 shrink-0 text-info" aria-hidden />
              <div>
                <p className="font-medium">Trace drill-down needs a newer nasiko-server</p>
                <p className="mt-1 text-xs text-muted-foreground">
                  This view uses the proposed <code>/finops/top-traces</code> endpoint, which this
                  server doesn&apos;t have yet. Run the mock dev server (<code>pnpm dev:mock</code>)
                  to preview it with mock data.
                </p>
              </div>
            </div>
          ) : error && !data ? (
            <PanelError error={error} onRetry={onRetry} what="traces" />
          ) : !data?.rows.length ? (
            <PanelEmpty title="No traces in this scope" />
          ) : (
            <ol className="flex flex-col divide-y divide-border">
              {data.rows.map((t) => {
                const href = traceLink(legacyUiUrl, t.session_id, t.trace_id);
                const when = fmtLocalDateTime(t.started_at);
                return (
                  <li
                    key={`${t.trace_id}:${t.agent_name}`}
                    className="flex flex-col gap-1 py-3 text-sm"
                  >
                    <div className="flex items-center justify-between gap-2">
                      <span className="truncate font-medium">{t.agent_name}</span>
                      <span className="font-semibold tabular-nums">{fmtMoney(t.cost_usd)}</span>
                    </div>
                    <div className="flex flex-wrap gap-x-3 gap-y-0.5 text-xs text-muted-foreground">
                      <span>{when}</span>
                      <span>{t.model ?? "unknown model"}</span>
                      <span className="tabular-nums">
                        {fmtTokens(t.input_tokens + t.output_tokens)} tokens
                      </span>
                      <span className="tabular-nums">{fmtLatency(t.latency_ms)}</span>
                      {t.cost_usd === 0 && t.input_tokens > 0 ? (
                        <span className="text-warning">unpriced</span>
                      ) : null}
                    </div>
                    {href ? (
                      <a
                        href={href}
                        target="_blank"
                        rel="noopener noreferrer"
                        className={buttonVariants({
                          variant: "link",
                          size: "sm",
                          className: "h-auto w-fit p-0 text-xs",
                        })}
                      >
                        Open trace
                        <span className="sr-only">
                          : {t.agent_name}, {when}, {fmtMoney(t.cost_usd)} (opens in a new tab)
                        </span>{" "}
                        <ExternalLink className="size-3" aria-hidden />
                      </a>
                    ) : null}
                  </li>
                );
              })}
            </ol>
          )}
          {data?.has_more ? (
            <p className="mt-2 text-xs text-muted-foreground">
              Showing the top {TOP_TRACES_LIMIT} by cost.
            </p>
          ) : null}
        </div>
      </SheetContent>
    </Sheet>
  );
}
