// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/components/RowsTable.tsx. Local edits:
// links are the navigation port's `Link` (`href`, `hrefFor` for a search); `asChild` is Base UI's `render` prop;
// `DataTable` is `VirtualDataTable` (`@nasiko/ui/table`); Prettier formatting.
/**
 * The home's rows (plans/feat-agent-centric.md PR2; design "Home", mockup A's dense table). Columns: name + kind +
 * owner, health with its reason, needs count, spend with its sparkline, budget use (a preview), saved tokens, last
 * change. Fleet groups the rows by owner, each group collapsible. Below 640 px the same rows are two lines each
 * (`PhoneRows`): name · kind · health, then spend · budget.
 */
import Link from "@nasiko/app-core/navigation";
import { hrefFor } from "@nasiko/app-core/url-state";
import { useState } from "react";
import {
  VirtualDataTable as DataTable,
  type VirtualDataTableColumn as DataTableColumn,
} from "@nasiko/ui/table";
import { Badge } from "@nasiko/ui/primitives";
import { displayStatus } from "../../agents/status";
import { RATED_STATUSES, type Rating } from "../../overview/health";
import { copy as overviewCopy } from "../../overview/copy";
import type { Range } from "../../overview/search";
import type { ResolvedWindow } from "../../tokenops/window";
import { fmtMoneyFloor, fmtTokens } from "@nasiko/ui/lib";
import { useBudgetStamp } from "../useBudgetStamp";
import type { HomeBudgets, SourceStatus } from "../api";
import { copy } from "../copy";
import {
  DEFAULT_SORT,
  ownerLine,
  rowGroups,
  type HomeRow,
  type RowGroup,
  type Sort,
  type SortKey,
} from "../rows";
import { TOUCH } from "../tuning";
import { BudgetUse, LastChangeCell, SpendCell } from "./RowCells";

const SORTABLE: Record<string, SortKey> = {
  name: "name",
  needs: "needs",
  spend: "spend",
  saved: "saved",
};

export interface RowsProps {
  rows: HomeRow[];
  /** Fleet groups rows by owner; Yours is one list. */
  grouped: boolean;
  days: number;
  range: Range;
  now: Date;
  win: Pick<ResolvedWindow, "start" | "end">;
  /** Draw spend sparklines (superusers: the series isn't ACL-scoped). */
  series: boolean;
  ratingsPending: boolean;
  savedStatus: SourceStatus;
  budgets: HomeBudgets;
  /** The owner's name, or null when the viewer can't name them. */
  ownerName: (id: string) => string | null;
  /** Calls in the window had no price: every spend sum is a floor (≥), as the strip says. */
  unpriced: boolean;
}

export function GroupLabel({ group, floor }: { group: RowGroup; floor: boolean }) {
  return (
    <span className="flex flex-wrap items-baseline gap-x-2">
      <span className="text-foreground">{group.label}</span>
      <span className="font-normal text-muted-foreground">
        {copy.groups.count(group.rows.length)}
        {group.spend !== null ? ` · ${fmtMoneyFloor(group.spend, floor)}` : ""}
      </span>
    </span>
  );
}

export function RowsTable(p: RowsProps & { sort: Sort; onSort: (s: Sort) => void }) {
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const groups = rowGroups(p.rows, p.grouped, p.ownerName);
  const byKey = new Map(groups?.map((g) => [g.key, g]) ?? []);
  const groupOf = new Map(groups?.flatMap((g) => g.rows.map((r) => [r.id, g] as const)) ?? []);
  const data = groups ? groups.flatMap((g) => g.rows) : p.rows;
  const budgetsOn = p.budgets.state !== "off";

  const columns: DataTableColumn<HomeRow>[] = [
    {
      id: "name",
      header: copy.table.name,
      // A value makes the column sortable (TanStack); the order itself comes from `sortRows`.
      accessorFn: (r) => r.displayName,
      // Long names (a harness's "Codex (someone@example.com)") wrap, so Last change still fits a 1440 px page.
      meta: { rowHeader: true, cellClassName: "min-w-40 max-w-60 whitespace-normal" },
      cell: ({ row }) => (
        <span className="flex min-w-0 flex-col">
          <span className="flex flex-wrap items-center gap-2">
            <Link
              href={`/agents/${encodeURIComponent(row.original.id)}`}
              className="font-medium underline-offset-4 hover:underline pointer-coarse:py-3"
            >
              {row.original.displayName}
            </Link>
            {row.original.harness ? <Badge variant="outline">{copy.table.harness}</Badge> : null}
          </span>
          <span className="text-xs text-muted-foreground">
            {ownerLine(row.original, p.ownerName)}
          </span>
        </span>
      ),
    },
    {
      id: "health",
      header: copy.table.health,
      enableSorting: false,
      cell: ({ row }) => <Health row={row.original} pending={p.ratingsPending} />,
    },
    {
      id: "needs",
      header: copy.table.needs,
      accessorFn: (r) => r.needs,
      meta: { numeric: true },
      cell: ({ row }) => (row.original.needs === null ? "—" : row.original.needs),
    },
    {
      id: "spend",
      header: copy.table.spend(p.days),
      accessorFn: (r) => r.spend,
      meta: { numeric: true },
      cell: ({ row }) => <SpendCell row={row.original} series={p.series} win={p.win} />,
    },
    ...(budgetsOn
      ? [
          {
            id: "budget",
            header: () => <BudgetHeader budgets={p.budgets} />,
            enableSorting: false,
            cell: ({ row }) => <BudgetUse row={row.original} state={p.budgets.state} />,
          } satisfies DataTableColumn<HomeRow>,
        ]
      : []),
    {
      id: "saved",
      header: copy.table.saved,
      accessorFn: (r) => r.saved,
      meta: { numeric: true },
      cell: ({ row }) =>
        row.original.saved !== null ? (
          fmtTokens(row.original.saved)
        ) : p.savedStatus === "ok" || p.savedStatus === "stale" ? (
          <span className="text-muted-foreground">{copy.strip.notReported}</span>
        ) : (
          "—"
        ),
    },
    {
      id: "lastChange",
      header: copy.table.lastChange,
      // Read per row on screen, so it can't sort: unread rows would sort as if unchanged (eng "sort by last change
      // disabled until loaded"; with reads only on screen, it never is for every row).
      enableSorting: false,
      cell: ({ row }) => <LastChangeCell row={row.original} now={p.now} range={p.range} />,
    },
  ];
  return (
    // Wider than the page: this box scrolls the columns, and takes focus so the keyboard can too.
    <div
      // A scrollable region must be focusable (axe scrollable-region-focusable).
      tabIndex={0}
      role="region"
      aria-label={copy.table.label}
      className="overflow-x-auto rounded-md contain-inline-size focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
    >
      <DataTable
        className="w-max min-w-full"
        label={copy.table.label}
        columns={columns}
        data={data}
        getRowId={(r) => r.id}
        sorting={[{ id: p.sort.key, desc: p.sort.desc }]}
        onSortingChange={(next) => {
          const s = next[0];
          const key = s ? SORTABLE[s.id] : undefined;
          p.onSort(key && s ? { key, desc: s.desc } : DEFAULT_SORT);
        }}
        groups={
          groups
            ? {
                of: (r) => groupOf.get(r.id)?.key ?? "",
                header: (key) => {
                  const g = byKey.get(key);
                  return g ? <GroupLabel group={g} floor={p.unpriced} /> : null;
                },
                collapsed,
                onToggle: (key) => {
                  setCollapsed((prev) => {
                    const next = new Set(prev);
                    if (!next.delete(key)) next.add(key);
                    return next;
                  });
                },
              }
            : undefined
        }
      />
    </div>
  );
}

/**
 * "Budget use", the preview badge, and when it was last read ("updated 2 minutes ago"). The badge and stamp are shown,
 * not announced: they'd be read with every budget cell as part of the column's name. The strip's note says both once.
 */
function BudgetHeader({ budgets }: { budgets: HomeBudgets }) {
  const updated = useBudgetStamp(budgets.updatedAt);
  return (
    <span className="flex flex-col items-start gap-0.5 py-1">
      {copy.table.budget}
      {budgets.preview ? (
        <Badge aria-hidden variant="outline" className="text-3xs">
          {copy.budget.preview}
        </Badge>
      ) : null}
      {updated ? (
        <span aria-hidden className="font-normal">
          {updated}
        </span>
      ) : null}
    </span>
  );
}

const BADGE: Record<Exclude<Rating, "unknown">, "success" | "warning" | "destructive"> = {
  healthy: "success",
  watch: "warning",
  action: "destructive",
};

export function Health({ row, pending }: { row: HomeRow; pending: boolean }) {
  const h = row.health;
  // Harnesses and agents that aren't deployed are never rated; a deployed agent whose inputs failed is Unknown.
  const rated = !row.harness && RATED_STATUSES.includes(displayStatus(row.status, false));
  if (rated && pending) return <span className="text-muted-foreground">{copy.table.checking}</span>;
  if (!h || h.rating === "unknown")
    return (
      <span className="text-muted-foreground">
        {rated && h ? overviewCopy.rating.unknown : copy.table.notRated}
      </span>
    );
  const reason = h.reasons[0]?.text;
  return (
    <span className="flex flex-col gap-0.5">
      {/* Each rating opens the Agents catalog filtered to it (the Overview's Fleet health counts did). */}
      <Badge
        variant={BADGE[h.rating]}
        className={`w-fit ${TOUCH}`}
        render={<Link href={hrefFor("/agents", { health: h.rating })} />}
      >
        {overviewCopy.rating[h.rating]}
      </Badge>
      {reason && h.rating !== "healthy" ? (
        <span className="text-xs text-muted-foreground">{reason}</span>
      ) : null}
    </span>
  );
}
