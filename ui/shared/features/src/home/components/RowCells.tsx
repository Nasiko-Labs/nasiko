// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/components/RowCells.tsx. Local edits:
// links are the navigation port's `Link` (`href`, `hrefFor` for a search); query keys are scoped (`productKey(scope,
// …)`; the scope from `useProductScope()`); Prettier formatting.
/**
 * The home's PR2b cells (plans/feat-agent-centric.md PR2, design "Home"): spend with its sparkline, budget use, and
 * last change. The two that read per row (series, builds) mount their query only once the row is on screen
 * (`useOnScreen`), through the page's limiter (`ROW_READS`); scrolling away before a read answers cancels it.
 */
import Link from "@nasiko/app-core/navigation";
import { hrefFor } from "@nasiko/app-core/url-state";
import { useProductScope } from "@nasiko/data-access/product";
import { useQueryClient } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { relTime } from "../../agents/format";
import { agentBuildsKey } from "../../deploy/api";
import { latestMarker, markersFor, markerText } from "../../hub/events";
import { HUB_DEFAULT_PRESET } from "../../hub/search";
import type { Range } from "../../overview/search";
import { resolveWindow, type ResolvedWindow } from "../../tokenops/window";
import { fmtMoney, fmtMoneyFloor, fmtShortDay } from "@nasiko/ui/lib";
import { budgetsShown, useRowBuilds, useRowSeries, type BudgetsState } from "../api";
import { copy } from "../copy";
import { budgetMonth, sparkPoints, type HomeRow } from "../rows";
import { SPARK_H, SPARK_W, TOUCH } from "../tuning";
import { useOnScreen } from "../useOnScreen";
import { Sparkline } from "./Sparkline";

type Win = Pick<ResolvedWindow, "start" | "end">;

export function SpendCell({
  row,
  series,
  win,
}: {
  row: HomeRow;
  /** Draw the sparkline: superusers only (the series isn't ACL-scoped; members get the total alone). */
  series: boolean;
  win: Win;
}) {
  if (row.spend === null) return "—";
  return (
    <span className="inline-flex items-center justify-end gap-2">
      {series && row.spend > 0 ? <SparkSlot id={row.id} win={win} /> : null}
      <SpendFigure row={row} />
    </span>
  );
}

/** The spend figure, with the harness estimate basis ("est."): the table's cell and the phone row's line. */
export function SpendFigure({ row }: { row: HomeRow }) {
  return (
    <span>
      {row.spend === null ? "—" : fmtMoney(row.spend)}
      {row.harness && (row.spend ?? 0) > 0 ? (
        <span className="ml-1 text-xs text-muted-foreground">{copy.table.estimate}</span>
      ) : null}
    </span>
  );
}

/** The sparkline's box, reserved before the series answers, so the columns never shift when it draws. */
function SparkSlot({ id, win }: { id: string; win: Win }) {
  const [ref, on] = useOnScreen<HTMLSpanElement>();
  return (
    <span ref={ref} className="inline-block shrink-0" style={{ width: SPARK_W, height: SPARK_H }}>
      {on ? <RowSpark id={id} win={win} /> : null}
    </span>
  );
}

function RowSpark({ id, win }: { id: string; win: Win }) {
  const q = useRowSeries(id, win);
  // Loading or failed: the total beside it stands alone (a sparkline is never the only place a number lives).
  return q.data ? <Sparkline values={sparkPoints(q.data, win)} /> : null;
}

/** "$640 of $1,000 · Oct", then the X5 line. Never says the budget stops or controls spend. */
export function BudgetUse({ row, state }: { row: HomeRow; state: BudgetsState }) {
  const b = row.budget;
  if (!budgetsShown(state)) return "—";
  // A budget counts routed calls; a coding harness's calls never go through the router (the hub hides its card too).
  if (row.harness) return <Quiet title={copy.budget.harness}>—</Quiet>;
  if (!b)
    return row.mine ? (
      <Quiet>{copy.budget.none}</Quiet>
    ) : (
      <Quiet title={copy.budget.onlyYours}>—</Quiet>
    );
  return (
    <span className="flex flex-col">
      <span className="tabular-nums">
        {copy.budget.use(
          fmtMoneyFloor(b.used, b.unpriced),
          fmtMoney(b.limit),
          budgetMonth(b.resetsAt),
        )}
      </span>
      {b.crossing ? (
        <span
          className={
            b.crossing.kind === "reached"
              ? "text-xs text-destructive"
              : "text-xs text-muted-foreground"
          }
        >
          {b.crossing.kind === "reached"
            ? copy.budget.reached
            : copy.budget.crosses(fmtShortDay(b.crossing.date))}
        </span>
      ) : null}
    </span>
  );
}

/**
 * The newest change the agent's build records show ("Build completed · v1.4.2 · 3 days ago"), linking to it on the
 * hub (`?marker=`) when it falls inside the window the hub opens on. "—" with the reason otherwise. The read is
 * observed only while the row is on screen, so scrolling away cancels one that hasn't answered; a row that already
 * has its records keeps showing them from the cache.
 */
export function LastChangeCell({ row, now, range }: { row: HomeRow; now: Date; range: Range }) {
  const [ref, on] = useOnScreen<HTMLSpanElement>();
  if (row.harness) return <Quiet title={copy.lastChange.harness}>—</Quiet>;
  return (
    <span ref={ref}>
      {on ? (
        <LastChange row={row} now={now} range={range} />
      ) : (
        <Kept row={row} now={now} range={range} />
      )}
    </span>
  );
}

function LastChange({ row, now, range }: { row: HomeRow; now: Date; range: Range }) {
  const q = useRowBuilds(row.id);
  if (q.isPending) return <Quiet>{copy.lastChange.loading}</Quiet>;
  if (q.isError) return <Quiet title={copy.lastChange.failed}>—</Quiet>;
  return <Change row={row} data={q.data} now={now} range={range} />;
}

/** Off screen: what the cache already holds, read without an observer (nothing is fetched or held open). */
function Kept({ row, now, range }: { row: HomeRow; now: Date; range: Range }) {
  const scope = useProductScope();
  const data = useQueryClient().getQueryData<RowBuilds>(agentBuildsKey(scope, row.id));
  return data ? (
    <Change row={row} data={data} now={now} range={range} />
  ) : (
    <Quiet title={copy.lastChange.notYet}>…</Quiet>
  );
}

type RowBuilds = NonNullable<ReturnType<typeof useRowBuilds>["data"]>;

function Change({
  row,
  data,
  now,
  range,
}: {
  row: HomeRow;
  data: RowBuilds;
  now: Date;
  range: Range;
}) {
  if (!data.available) return <Quiet title={copy.lastChange.hidden}>—</Quiet>;
  const m = latestMarker(
    markersFor({
      builds: data.value,
      versions: null,
      crashedAt: null,
      events: null,
      // No upper bound: a build that finished after this visit's frozen "now" is still the newest change.
      win: { start: new Date(0), end: new Date(8.64e15) },
      harness: false,
    }),
  );
  if (!m) return <Quiet title={copy.lastChange.none}>—</Quiet>;
  // The hub opens on 7d, or on its own default; a marker outside that window would be dropped there, so it isn't sent.
  const preset = range === "7d" ? ("7d" as const) : undefined;
  const inHub =
    m.at >= resolveWindow({ preset: preset ?? HUB_DEFAULT_PRESET }, now).start.getTime();
  return (
    <Link
      href={hrefFor(
        `/agents/${encodeURIComponent(row.id)}`,
        inHub ? { marker: m.id, preset } : { preset },
      )}
      className={`flex flex-col justify-center underline-offset-4 hover:underline ${TOUCH}`}
    >
      <span>{markerText(m)}</span>
      <span className="text-xs text-muted-foreground">
        {relTime(new Date(m.at).toISOString(), Math.max(now.getTime(), m.at))}
      </span>
    </Link>
  );
}

/** A muted cell; with a `title`, the reason is read out too (a tooltip alone isn't announced). */
export function Quiet({ children, title }: { children: ReactNode; title?: string }) {
  return (
    <span className="text-muted-foreground" title={title}>
      {title ? (
        <>
          <span aria-hidden>{children}</span>
          <span className="sr-only">{title}</span>
        </>
      ) : (
        children
      )}
    </span>
  );
}
