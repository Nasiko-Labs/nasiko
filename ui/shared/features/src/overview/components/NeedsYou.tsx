// Ported from nasiko-ui-lab@1b99bdda common/src/features/overview/components/NeedsYou.tsx. Local edits: the lab's `EmptyState` (shared/state-card) is `LabEmptyState`; `"use client"`;
// each row's `Action` (`Button asChild` around a TanStack `Link`) is a `ButtonLink` taking the link's props (hrefs:
// `chatHref(id)`, `/agents/<id>`, and `pageHref` for the router's budgets hash and the Sessions filter); the request
// row's `state={{ waitingRequest }}` (the navigation port's links carry no state) is chat/waitingNav.ts `openFromWaiting(sessionId,
// requestId)` on click, as Chat's own Waiting rows do; optional props admit `undefined`; Prettier formatting.
"use client";

/**
 * Needs you (plans/feat-overview.md §6): the inbox, most urgent first. Severity is said once, by a plain icon beside
 * the text (design review 9A); five rows show and the rest scroll inside the card (2A); the header always says when
 * it last checked (8A); a failed source adds a line and never turns into "Nothing needs you" (5A).
 */
import { OpenFlowLink } from "../../flows/components/OpenFlowLink";
import { CircleAlert, Hand, TriangleAlert } from "lucide-react";
import { useEffect, useRef, useState, type ComponentProps, type ReactNode } from "react";
import { LabEmptyState as EmptyState } from "@nasiko/ui/composites";
import { ButtonLink } from "../../agents/components/ButtonLink";
import { ScrollArea } from "@nasiko/ui/primitives";
import { Skeleton } from "@nasiko/ui/primitives";
import { relTime } from "../../agents/format";
import { chatHref } from "../../chat/nav";
import { rememberRailView } from "../../chat/rememberTarget";
import { openFromWaiting } from "../../chat/waitingNav";
import { cn } from "@nasiko/ui/cn";
import type { NeedsYou as NeedsYouData } from "../api";
import { copy } from "../copy";
import type { NeedRow, Severity } from "../needs";
import { CHECKED_TICK_MS, LAST_WEEK, NEEDS_VISIBLE_ROWS } from "../tuning";
import { Card, CardError, CardSkeleton, pageHref, SourceFailed, TOUCH } from "./Card";

/** `detail` colours the row's reason: action and watch say why in their own tone; a waiting request stays quiet. */
const ICON: Record<
  Severity,
  { Icon: typeof CircleAlert; tone: string; detail: string; label: string }
> = {
  action: {
    Icon: CircleAlert,
    tone: "text-destructive",
    detail: "text-destructive",
    label: copy.rating.action,
  },
  watch: {
    Icon: TriangleAlert,
    tone: "text-warning",
    detail: "text-warning",
    label: copy.rating.watch,
  },
  waiting: {
    Icon: Hand,
    tone: "text-info",
    detail: "text-muted-foreground",
    label: copy.needs.waiting,
  },
};
/** A row's one action: a small outline button that is a link. */
function Action(props: Omit<ComponentProps<typeof ButtonLink>, "variant" | "size" | "className">) {
  return (
    <ButtonLink variant="outline" size="sm" className={cn("shrink-0 text-xs", TOUCH)} {...props} />
  );
}

export function NeedsYou({
  data,
  now,
  userId,
  pausedFlows,
  className,
}: {
  data: NeedsYouData;
  now: number;
  userId: string;
  /** Chat session → its paused flow (plans/feat-flows.md O2, amended 2026-10-06): a request row links it. */
  pausedFlows?: ReadonlyMap<string, string> | undefined;
  className?: string | undefined;
}) {
  const titleRef = useRef<HTMLHeadingElement>(null);
  const { needs } = data;
  // Every source the server has failed: one card error (on OSS, budgets are absent, so three of three count).
  const allFailed =
    !needs.rows.length &&
    !needs.loading &&
    needs.failed.length > 0 &&
    needs.failed.length === needs.available;
  // "Checked …" ages on an open tab: the page's `now` is frozen per visit (/ship adversarial).
  const [clock, setClock] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => {
      setClock(Date.now());
    }, CHECKED_TICK_MS);
    return () => {
      clearInterval(t);
    };
  }, []);
  const ageNow = Math.max(now, clock);
  const count = { action: 0, watch: 0, waiting: 0 };
  for (const r of needs.rows) count[r.severity]++;
  return (
    <Card
      id="overview-needs"
      title={copy.needs.title}
      titleClassName="text-lg tracking-tight"
      // Anything that needs action lifts the card: a destructive edge along the top (live bolder pass).
      className={cn(
        "gap-3.5",
        className,
        count.action > 0 && "border-destructive/35 shadow-[inset_0_3px_0_var(--color-destructive)]",
      )}
      meta={
        needs.rows.length ? (
          <span className="inline-flex gap-3 font-medium">
            {(["action", "watch", "waiting"] as const).map((s) =>
              count[s] ? (
                <span key={s} className={ICON[s].tone}>
                  {copy.needs.count[s](count[s])}
                </span>
              ) : null,
            )}
          </span>
        ) : undefined
      }
      aside={
        data.lastChecked
          ? copy.needs.checked(
              relTime(new Date(data.lastChecked).toISOString(), Math.max(ageNow, data.lastChecked)),
            )
          : undefined
      }
      titleRef={titleRef}
    >
      {allFailed ? (
        <CardError
          what={copy.needs.what}
          onRetry={() => {
            Object.values(data.retry).forEach((r) => {
              r();
            });
          }}
          titleRef={titleRef}
        />
      ) : needs.loading && !needs.rows.length ? (
        <CardSkeleton />
      ) : (
        <div className="flex min-h-0 flex-1 flex-col gap-2">
          {needs.empty ? (
            <div data-testid="needs-empty">
              <EmptyState title={copy.needs.nothing} className="py-6 md:py-6" />
            </div>
          ) : needs.rows.length ? (
            // Five rows show; the list scrolls inside the card after that (design review 2A).
            // Taller on touch, where row actions are 44 px (/ship review): five rows still fit without clipping.
            <ScrollArea className="[&>[data-slot=scroll-area-viewport]]:max-h-70 pointer-coarse:[&>[data-slot=scroll-area-viewport]]:max-h-84 [&>[data-slot=scroll-area-viewport]>div]:block!">
              <ul className="divide-y divide-border pr-2.5" aria-label={copy.needs.title}>
                {needs.rows.map((r) => (
                  <li key={r.key}>
                    <Row
                      row={r}
                      now={now}
                      userId={userId}
                      flowId={r.kind === "request" ? pausedFlows?.get(r.chat.sessionId) : undefined}
                    />
                  </li>
                ))}
              </ul>
            </ScrollArea>
          ) : null}
          {/* Past five rows the list scrolls; say so, a clipped row alone is easy to miss (QA ISSUE-002). */}
          {needs.rows.length > NEEDS_VISIBLE_ROWS ? (
            <p className="text-xs text-muted-foreground" data-testid="needs-more">
              {copy.needs.more(needs.rows.length - NEEDS_VISIBLE_ROWS)}
            </p>
          ) : null}
          {needs.loading && needs.rows.length ? (
            <Skeleton className="h-8 w-full motion-reduce:animate-none" aria-label={copy.loading} />
          ) : null}
          {needs.failed.map((id) => (
            <SourceFailed key={id} what={copy.needs.source[id]} onRetry={data.retry[id]} />
          ))}
        </div>
      )}
    </Card>
  );
}

function Row({
  row,
  now,
  userId,
  flowId,
}: {
  row: NeedRow;
  now: number;
  userId: string;
  /** The request's paused flow, when one matches its chat. */
  flowId?: string | undefined;
}) {
  const { Icon, tone, detail: detailTone, label } = ICON[row.severity];
  let title: string;
  let detail: string;
  let action: ReactNode;
  switch (row.kind) {
    case "request":
      title = copy.needs.request(row.chat.kind, row.chat.count);
      detail = `${row.chat.chatTitle} · ${relTime(row.chat.at, Math.max(now, Date.parse(row.chat.at) || now))}`;
      action = (
        <Action
          href={chatHref(row.chat.sessionId)}
          onClick={() => {
            openFromWaiting(row.chat.sessionId, row.chat.firstId);
          }}
          aria-label={copy.needs.reviewLabel(row.chat.chatTitle)}
        >
          {copy.needs.review}
        </Action>
      );
      break;
    case "outside":
      title = copy.needs.outside(row.count);
      detail = copy.needs.outsideDetail;
      // Chat's Waiting view is the one that explains requests outside Chat: pick it before opening Chat (/ship review).
      action = (
        <Action
          href="/chat"
          onClick={() => {
            rememberRailView(userId, "waiting");
          }}
          aria-label={copy.needs.openWaiting}
        >
          {copy.needs.openChat}
        </Action>
      );
      break;
    case "agent":
      title = row.name;
      detail = row.reason;
      action = row.budget ? (
        <Action
          href={pageHref("/router", {}, "router-budgets")}
          aria-label={copy.needs.budgetsLabel(row.name)}
        >
          {copy.needs.budgets}
        </Action>
      ) : (
        <Action
          href={`/agents/${encodeURIComponent(row.id)}`}
          aria-label={copy.needs.openAgentLabel(row.name)}
        >
          {copy.needs.openAgent}
        </Action>
      );
      break;
    case "budget":
      title = copy.needs.budget(row.label, row.state, row.crossed);
      detail = copy.needs.budgetDetail;
      action = (
        <Action
          href={pageHref("/router", {}, "router-budgets")}
          aria-label={copy.needs.budgetsLabel(row.label)}
        >
          {copy.needs.budgets}
        </Action>
      );
      break;
    case "sessions":
      title = copy.needs.sessions(row.failed);
      detail = row.agents.join(", ");
      action = (
        <Action
          href={pageHref("/sessions", { ...LAST_WEEK, lane: "failing" })}
          aria-label={copy.needs.seeFailing}
        >
          {copy.needs.seeSessions}
        </Action>
      );
      break;
  }
  return (
    <div className="flex items-center gap-3 py-2.5" data-kind={row.kind}>
      {/* The icon sits on a tile of its own tone. */}
      <Icon
        className={cn("box-content size-4 shrink-0 rounded-md bg-current/12 p-1.5", tone)}
        aria-label={label}
        role="img"
      />
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-semibold">{title}</p>
        {detail ? <p className={cn("truncate text-xs", detailTone)}>{detail}</p> : null}
      </div>
      {flowId ? (
        <OpenFlowLink flowId={flowId} variant="ghost" className="shrink-0 text-xs" />
      ) : null}
      {action}
    </div>
  );
}
