// Ported from nasiko-ui-lab@1b99bdda common/src/features/optimization/components/Lead.tsx. Local edits: `Button
// asChild` around the Chat link -> `ButtonLink` (agents/components/ButtonLink.tsx); the TokenOps link is the navigation port's `Link` (`@nasiko/app-core/navigation`)
// with `hrefFor('/tokenops', …)`; optional props typed `| undefined`; Prettier formatting.
/**
 * The /optimization lead (plans/feat-optimization-page.md R1A, R1B, R2A, R2B, R2E, R4B, R5A, R7A; eng C1–C3, C5, C6):
 * an unboxed section whose h2 is the answer, then the five-number sentence with the TokenOps link, coverage, cost
 * coverage, the trend and the latest settings change (described, never as a cause), then the chart. It loads and
 * fails on its own (R2C); every number comes from reports, never from switch counts (C1).
 */
import Link from "@nasiko/app-core/navigation";
import type { ReactNode } from "react";
import { PanelError, PanelSkeleton } from "@nasiko/ui/composites";
import { Skeleton } from "@nasiko/ui/primitives";
import { hrefFor } from "@nasiko/app-core/url-state";
import { ButtonLink } from "../../agents/components/ButtonLink";
import type { ResolvedWindow } from "../../tokenops/window";
import { fmtMoney, fmtShortDay } from "@nasiko/ui/lib";
import { copy } from "../copy";
import {
  latestChange,
  pctOf,
  savedCost,
  savedOf,
  tokensShort,
  trendOf,
  TREND_DAYS,
  type BucketSize,
  type LeadState,
} from "../lead";
import type { SettingsChange } from "../types";
import { Outline } from "./Outline";
import { SavingsTrend, type LabelledRow } from "./SavingsTrend";

export function Lead({
  state,
  error,
  onRetry,
  win,
  superuser,
  rows,
  tableRows,
  unit,
  tableUnit,
  under,
  history,
  compression,
  onSelect,
}: {
  state: LeadState;
  error: unknown;
  onRetry: () => void;
  win: Pick<ResolvedWindow, "label" | "start" | "preset"> & {
    from?: string | undefined;
    to?: string | undefined;
  };
  superuser: boolean;
  rows: LabelledRow[];
  tableRows: LabelledRow[];
  unit: BucketSize;
  tableUnit: BucketSize;
  /** R6B: what sits under the lead's words on a phone (the Your settings link). */
  under?: ReactNode | undefined;
  history: SettingsChange[] | undefined;
  /** C1: history compression for your chats, or null when Needs attention says it (or it isn't known yet). */
  compression: string | null;
  onSelect: (iso: string, size: BucketSize) => void;
}) {
  const hourly = win.preset === "24h";
  const heading =
    state.kind === "loading"
      ? null
      : state.kind === "error"
        ? copy.lead.heading.error
        : state.kind === "absent"
          ? copy.lead.heading.absent
          : state.kind === "no-agents"
            ? copy.lead.heading.noAgents
            : state.kind === "no-reports"
              ? copy.lead.heading.noReports
              : state.kind === "low-coverage"
                ? copy.lead.heading.lowCoverage
                : state.share > 0
                  ? copy.lead.heading.saving(pctOf(state.share))
                  : copy.lead.heading.none;

  return (
    <section
      data-tour="optimization-lead"
      aria-labelledby="optimization-lead-h"
      aria-busy={state.kind === "loading"}
      className="flex flex-col gap-3"
    >
      <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
        {heading ? (
          // The error panel below says what failed; the heading stays for navigation only (review: design).
          <h2
            id="optimization-lead-h"
            className={state.kind === "error" ? "sr-only" : "text-lg font-semibold"}
          >
            {heading}
          </h2>
        ) : (
          <>
            <h2 id="optimization-lead-h" className="sr-only">
              {copy.lead.chart.loading}
            </h2>
            <Skeleton aria-hidden className="h-6 w-64" />
          </>
        )}
        <span className="text-xs text-muted-foreground">
          {copy.lead.meta(win.label, superuser)}
        </span>
      </div>
      <Body
        state={state}
        error={error}
        onRetry={onRetry}
        win={win}
        superuser={superuser}
        history={history}
      />
      {under}
      {compression && state.kind !== "loading" && state.kind !== "error" ? (
        <p className="m-0 text-xs text-muted-foreground">{compression}</p>
      ) : null}
      {state.kind === "saving" || state.kind === "low-coverage" ? (
        <SavingsTrend
          rows={rows}
          tableRows={tableRows}
          unit={unit}
          tableUnit={tableUnit}
          period={win.label}
          onSelect={onSelect}
        />
      ) : state.kind === "loading" ? (
        <PanelSkeleton height={240} />
      ) : state.kind === "error" ? null : (
        // R2B: the chart's place and shape, static (never read as loading), saying what will show here.
        <Outline>
          {state.kind === "absent"
            ? copy.lead.chart.outline(hourly)
            : state.kind === "no-reports"
              ? copy.lead.noReportsLine
              : copy.lead.noAgentsLine}
        </Outline>
      )}
    </section>
  );
}

function Body({
  state,
  error,
  onRetry,
  win,
  superuser,
  history,
}: {
  state: LeadState;
  error: unknown;
  onRetry: () => void;
  win: Pick<ResolvedWindow, "start" | "preset"> & {
    from?: string | undefined;
    to?: string | undefined;
  };
  superuser: boolean;
  history: SettingsChange[] | undefined;
}) {
  if (state.kind === "loading") return <Skeleton aria-hidden className="h-5 w-full max-w-xl" />;
  if (state.kind === "error")
    return <PanelError error={error} onRetry={onRetry} what={copy.lead.chart.what} />;
  if (state.kind === "absent")
    return <p className="m-0 text-sm text-muted-foreground">{copy.lead.absentLine}</p>;
  if (state.kind === "no-reports")
    return (
      <div>
        <ButtonLink href="/chat" size="sm" variant="outline" className="pointer-coarse:min-h-11">
          {copy.lead.startChat}
        </ButtonLink>
      </div>
    );
  if (state.kind === "no-agents") return null;
  const s = state.savings;
  const coverage = state.coverage;
  const cost = savedCost(s);
  const saved = savedOf(s);
  const trend = state.kind === "saving" ? trendOf(s.series) : null;
  const change = latestChange(history);
  const recordedLate =
    s.recorded_since && Date.parse(s.recorded_since) > win.start.getTime()
      ? copy.savings.coverage(fmtShortDay(s.recorded_since))
      : null;
  const notes = [
    coverage ? copy.lead.basedOn(coverage.reports, coverage.eligible, pctOf(coverage.share)) : null,
    s.priced_reports !== undefined && cost !== null && s.priced_reports < s.reports
      ? copy.lead.costCovers(s.priced_reports, s.reports)
      : null,
    recordedLate,
    trend ? copy.lead.trend(trend.direction, pctOf(trend.from), pctOf(trend.to), TREND_DAYS) : null,
    change ? copy.lead.change(change.field, changeValue(change), fmtShortDay(change.at)) : null,
  ].filter(Boolean);
  return (
    <div className="flex flex-col gap-1">
      <p className="m-0 text-sm">
        {copy.lead.sentence(
          superuser,
          tokensShort(s.sent_tokens),
          tokensShort(s.pool_tokens),
          tokensShort(saved),
          cost !== null ? fmtMoney(cost) : null,
          state.kind === "saving" ? pctOf(state.share) : null,
        )}{" "}
        <Link
          href={hrefFor("/tokenops", { preset: win.preset, from: win.from, to: win.to })}
          className="underline underline-offset-4 hover:text-primary-text"
        >
          {copy.lead.tokenops}
        </Link>
      </p>
      {notes.length ? <p className="m-0 text-xs text-muted-foreground">{notes.join(" ")}</p> : null}
    </div>
  );
}

/** CX-H values are the server's enum words; show them as the settings form names them. */
function changeValue(c: SettingsChange): string {
  if (c.field === "strategy")
    return Object.hasOwn(copy.strategies, c.to)
      ? copy.strategies[c.to as keyof typeof copy.strategies].name
      : c.to;
  if (c.field === "level")
    return c.to === "low" || c.to === "medium" || c.to === "high" ? copy.level(c.to) : c.to;
  return c.to;
}
