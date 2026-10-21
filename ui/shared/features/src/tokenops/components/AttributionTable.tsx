// Ported from nasiko-ui-lab@1b99bdda common/src/features/tokenops/components/AttributionTable.tsx. Local edits:
// `DataTable`/`DataTableColumn` are `VirtualDataTable`/`VirtualDataTableColumn` from `@nasiko/ui/table` (the lab's
// table on TanStack Table 8); Base UI's ToggleGroup is value-array based (`value={[view]}`, `onValueChange` reads
// `v[0]`) and the Select's `onValueChange` is `(value, eventDetails)` and it takes `items` so its closed Value
// shows labels; `Card asChild` -> `Card render={<li …/>}`;
// primitives from `@nasiko/ui/primitives`, composites from `@nasiko/ui/composites`, `cn`/formatters/`POLARITY`/
// `prefersReducedMotion` from `@nasiko/ui/cn` and `@nasiko/ui/lib`; `AgentLink` from the ported agents feature;
// optional props typed `| undefined`; lint-driven mechanical edits (void arrow bodies braced,
// unnecessary assertions dropped); on development (3aac84f66, 49950eb1e and d2e0d4bcf, for GenUI) the table takes an
// optional `pageSize` (a `Pager` under it) and `onSelect` (a focus toggle per row; `aria-pressed:` for Base UI's
// Toggle); Prettier formatting.
// The redesign (legacy ui/ on development, 3aac84f66: 8bf09bf29): the top 10 rows behind Show all (not with a
// `pageSize`), and Group by as `Segmented`.
"use client";

/**
 * F3 — "Who or what is driving cost?" (plan A1, A2b, A7, A9, A11, A12, A14, A18).
 *
 * Rows come from the ACL-scoped dashboard (current + previous window), joined by id.
 * Sort is a URL param (history replace); search is a URL param (replace). CSV exports
 * exactly the visible rows in the current sort, with formula-safe cells.
 * On narrow screens the table becomes a card list.
 */
import { Crosshair, Download, ListTree } from "lucide-react";
import { createContext, use, useEffect, useMemo, useRef, useState } from "react";
import {
  Badge,
  Button,
  Card,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Toggle,
} from "@nasiko/ui/primitives";
import { AgentLink } from "../../agents/components/AgentLink";
import { cn } from "@nasiko/ui/cn";
import { searchRows, sortRows, type AttributionRow } from "../attribution";
import { downloadCsv, toCsv, type CsvColumn } from "../csv";
import {
  fmtCostPerOp,
  fmtHours,
  fmtInt,
  fmtLatency,
  fmtMoney,
  fmtPct,
  fmtTokens,
  pageCount,
  pageOf,
  POLARITY,
  prefersReducedMotion,
} from "@nasiko/ui/lib";
import type { SortKey } from "../search";
import {
  Delta,
  Panel,
  PanelEmpty,
  PanelError,
  PanelSkeleton,
  Pager,
  SearchInput,
} from "@nasiko/ui/composites";
import {
  VirtualDataTable as DataTable,
  type VirtualDataTableColumn as DataTableColumn,
} from "@nasiko/ui/table";

import { Segmented } from "./Segmented";

const VIEWS = [
  { value: "agent", label: "Agents" },
  { value: "workflow", label: "Workflows" },
] as const;

const SORT_LABELS: Record<SortKey, string> = {
  cost: "Highest spend",
  tokens: "Most tokens",
  operations: "Most operations",
  latency: "Slowest",
  hours: "Most container hours",
  name: "Name",
};

/** Rows shown before Show all. */
const ROW_CAP = 10;

const HIGHLIGHT = "bg-primary/10 ring-1 ring-primary/40";

export function AttributionTable({
  rows,
  view,
  sort,
  q,
  loading,
  error,
  onRetry,
  prevUnavailable,
  highlightId,
  agentFilter,
  onView,
  onSort,
  onSearch,
  onViewTraces,
  onSelect,
  onLast30,
  unpriced,
  pageSize,
}: {
  rows: AttributionRow[] | undefined;
  view: "agent" | "workflow";
  sort: SortKey;
  q?: string | undefined;
  loading: boolean;
  error: unknown;
  onRetry: () => void;
  prevUnavailable: boolean;
  highlightId?: string | undefined;
  agentFilter?: string | undefined;
  onView: (v: "agent" | "workflow") => void;
  onSort: (s: SortKey) => void;
  onSearch: (q: string) => void;
  onViewTraces: (agentId: string) => void;
  onSelect?: ((agentId: string) => void) | undefined;
  /** Offered in the empty-window state; undefined when already on 30d (A2). */
  onLast30?: (() => void) | undefined;
  unpriced: boolean;
  pageSize?: number | undefined;
}) {
  const visible = useMemo(() => sortRows(searchRows(rows ?? [], q), sort), [rows, q, sort]);
  // The top rows answer "who"; the long tail waits behind Show all, so the section stays one screen. A search or a
  // highlight from the scatter below the fold shows everything (CSV always exports every visible row).
  const [expanded, setExpanded] = useState(false);
  const highlightBelow =
    !!highlightId && visible.slice(0, ROW_CAP).every((r) => r.id !== highlightId);
  const all = expanded || !!q || highlightBelow || visible.length <= ROW_CAP + 2;
  const firstPage = () => {
    const at = highlightId ? visible.findIndex((r) => r.id === highlightId) : -1;
    return pageSize && at >= 0 ? Math.floor(at / pageSize) : 0;
  };
  const [page, setPage] = useState(firstPage);
  const [pagedFor, setPagedFor] = useState({ visible, highlightId });
  if (pagedFor.visible !== visible || pagedFor.highlightId !== highlightId) {
    setPagedFor({ visible, highlightId });
    setPage(firstPage());
  }
  const pages = pageSize ? pageCount(visible.length, pageSize) : 1;
  const current = Math.min(page, pages - 1);
  // A page size (GenUI's views) pages the rows; without one, the top rows and Show all.
  const shown = pageSize
    ? pageOf(visible, current, pageSize)
    : all
      ? visible
      : visible.slice(0, ROW_CAP);
  const noun = view === "agent" ? "Agent" : "Workflow";
  const sortOptions = (Object.keys(SORT_LABELS) as SortKey[]).filter(
    (s) => view === "agent" || s !== "hours",
  );

  const columns: CsvColumn<AttributionRow>[] = [
    { header: noun, value: (r) => r.name },
    { header: "Spend (USD)", value: (r) => r.cost },
    { header: "Share (%)", value: (r) => Math.round(r.sharePct * 10) / 10 },
    {
      header: "Change vs previous (%)",
      value: (r) => (r.deltaPct === null ? null : Math.round(r.deltaPct * 10) / 10),
    },
    { header: "Tokens", value: (r) => r.tokens },
    { header: view === "agent" ? "Operations" : "Executions", value: (r) => r.operations },
    { header: "Cost per operation (USD)", value: (r) => r.costPerOp },
    { header: `${view === "agent" ? "p50" : "Average"} latency (ms)`, value: (r) => r.latency },
    {
      header: "Cache-read ratio (%)",
      value: (r) => (r.cacheRatioPct === null ? null : Math.round(r.cacheRatioPct * 10) / 10),
    },
    ...(view === "agent"
      ? [{ header: "Container hours", value: (r: AttributionRow) => r.containerHours }]
      : []),
  ];

  return (
    <Panel
      title="Who is driving cost"
      subtitle={`Share and change vs the previous period of equal length · source: agent dashboard (agents you can access)${unpriced ? " · excludes unpriced calls" : ""}`}
      labelledBy="attribution-title"
      actions={
        <>
          <Segmented label="Group by" value={view} options={VIEWS} onChange={onView} />
          <Select
            value={sort}
            onValueChange={(v: string | null) => {
              onSort(v as SortKey);
            }}
            // Base UI's Value shows a label only for items it knows (the lab's Radix Value read the rendered item).
            items={Object.fromEntries(sortOptions.map((s) => [s, SORT_LABELS[s]]))}
          >
            <SelectTrigger size="sm" className="w-44" aria-label="Sort by">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {sortOptions.map((s) => (
                <SelectItem key={s} value={s}>
                  {SORT_LABELS[s]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <SearchInput
            value={q ?? ""}
            onChange={(e) => {
              onSearch(e.target.value);
            }}
            placeholder={`Search ${noun.toLowerCase()}s…`}
            aria-label={`Search ${noun.toLowerCase()}s`}
            className="h-8 w-40"
          />
          <Button
            variant="outline"
            size="sm"
            disabled={!visible.length}
            onClick={() => {
              downloadCsv(`tokenops-${view}s.csv`, toCsv(columns, visible));
            }}
          >
            <Download className="size-4" aria-hidden /> CSV
          </Button>
        </>
      }
    >
      {loading && !rows ? (
        <PanelSkeleton height={240} />
      ) : error && !rows ? (
        <PanelError error={error} onRetry={onRetry} what="cost attribution" />
      ) : !rows?.length ? (
        agentFilter ? (
          <PanelEmpty title="This agent isn't in the current agent list">
            It may be deleted or not accessible to you. Its spend still appears in the calendar and
            timeline, which are fleet-wide.
          </PanelEmpty>
        ) : (
          <PanelEmpty title={`No ${noun.toLowerCase()} activity in this window`}>
            {onLast30 ? (
              <Button variant="outline" size="sm" className="mt-2" onClick={onLast30}>
                Show last 30 days
              </Button>
            ) : null}
          </PanelEmpty>
        )
      ) : !visible.length ? (
        <PanelEmpty title={`No ${noun.toLowerCase()}s match “${q}”`} />
      ) : (
        <>
          <DesktopTable
            rows={shown}
            view={view}
            sort={sort}
            prevUnavailable={prevUnavailable}
            highlightId={highlightId}
            onViewTraces={onViewTraces}
            onSelect={onSelect}
          />
          <MobileCards
            rows={shown}
            view={view}
            prevUnavailable={prevUnavailable}
            highlightId={highlightId}
            onViewTraces={onViewTraces}
            onSelect={onSelect}
          />
          {!pageSize && visible.length > ROW_CAP + 2 && !q && !highlightBelow ? (
            <Button
              variant="ghost"
              size="sm"
              className="w-fit text-xs"
              aria-expanded={expanded}
              onClick={() => {
                setExpanded((v) => !v);
              }}
            >
              {expanded ? `Show top ${ROW_CAP}` : `Show all ${visible.length}`}
            </Button>
          ) : null}
          <Pager page={current} pages={pages} onPage={setPage} />
        </>
      )}
    </Panel>
  );
}

type Col = DataTableColumn<AttributionRow>;
const NUM = { numeric: true, headerClassName: "px-1.5 2xl:px-2", cellClassName: "px-1.5 2xl:px-2" };

/**
 * What cells need beyond their row. FlexRender renders each `cell` as a component type, so column
 * defs must keep their identity across renders or every cell remounts (and a focused Traces button
 * is lost when the drawer closes): the defs live at module level and read these through context.
 */
const CellCtx = createContext<{
  prevUnavailable: boolean;
  onViewTraces: (id: string) => void;
  onSelect?: ((id: string) => void) | undefined;
  highlightId?: string | undefined;
}>({
  prevUnavailable: true,
  onViewTraces: () => {},
});

function DeltaCell({ row }: { row: AttributionRow }) {
  return <RowDelta row={row} prevUnavailable={use(CellCtx).prevUnavailable} />;
}

function FocusToggle({ row: r }: { row: AttributionRow }) {
  const { onSelect, highlightId } = use(CellCtx);
  if (!onSelect) return null;
  const on = r.id === highlightId;
  return (
    <Toggle
      size="sm"
      pressed={on}
      onPressedChange={() => {
        onSelect(on ? "" : r.id);
      }}
      className="h-7 px-2 aria-pressed:bg-primary/10 aria-pressed:text-primary-text"
      aria-label={on ? `Stop focusing on ${r.name}` : `Focus on ${r.name}`}
      title={on ? "In focus" : "Focus"}
    >
      <Crosshair className="size-3.5" aria-hidden />
    </Toggle>
  );
}

function TracesCell({ row: r }: { row: AttributionRow }) {
  const { onViewTraces } = use(CellCtx);
  // Icon-only: the 11-column table doesn't fit its 8/12 slot with a label (aria-label names it).
  return (
    <Button
      variant="ghost"
      size="sm"
      className="h-7 px-2 text-xs"
      onClick={() => {
        onViewTraces(r.id);
      }}
      aria-label={`View traces for ${r.name}`}
      title="View traces"
    >
      <ListTree className="size-3.5" aria-hidden />
    </Button>
  );
}

/** Column ids are the SortKey the column is ordered by, so `sorting` marks aria-sort on it whatever its label. */
function columnsFor(view: "agent" | "workflow"): Col[] {
  return [
    {
      id: "name",
      header: view === "agent" ? "Agent" : "Workflow",
      meta: { cellClassName: "max-w-55" },
      cell: ({ row: { original: r } }) => (
        <div className="flex items-center gap-1.5">
          {view === "agent" ? (
            <AgentLink id={r.id} name={r.name} className="truncate font-medium">
              {r.name}
            </AgentLink>
          ) : (
            <span className="truncate font-medium">{r.name}</span>
          )}
          {r.capped ? (
            <Badge
              variant="outline"
              className="text-2xs"
              title="Operations were capped by the aggregation limit; totals undercount."
            >
              ~approx
            </Badge>
          ) : null}
        </div>
      ),
    },
    { id: "cost", header: "Spend", meta: NUM, cell: ({ row }) => fmtMoney(row.original.cost) },
    {
      id: "share",
      header: "Share",
      meta: NUM,
      cell: ({ row: { original: r } }) => (
        <div className="flex items-center justify-end gap-2">
          <div className="hidden h-1.5 w-12 rounded-full bg-muted 2xl:block" aria-hidden>
            <div
              className="h-1.5 rounded-full bg-chart-1-edge"
              style={{ width: `${Math.min(100, r.sharePct)}%` }}
            />
          </div>
          <span>{fmtPct(r.sharePct)}</span>
        </div>
      ),
    },
    {
      id: "delta",
      header: () => <span title="Change vs previous period">Δ</span>,
      meta: NUM,
      cell: ({ row }) => <DeltaCell row={row.original} />,
    },
    {
      id: "tokens",
      header: "Tokens",
      meta: NUM,
      cell: ({ row }) => fmtTokens(row.original.tokens),
    },
    {
      id: "operations",
      header: view === "agent" ? "Operations" : "Executions",
      meta: NUM,
      cell: ({ row }) => fmtInt(row.original.operations),
    },
    {
      id: "costPerOp",
      header: "Cost/op",
      meta: NUM,
      cell: ({ row }) => fmtCostPerOp(row.original.costPerOp),
    },
    // Agents carry p50; workflows carry the server's arithmetic mean (WorkflowFinopsRow.avg_latency_ms).
    {
      id: "latency",
      header: view === "agent" ? "p50" : "Avg latency",
      meta: NUM,
      cell: ({ row }) => fmtLatency(row.original.latency),
    },
    {
      id: "cache",
      header: "Cache read",
      meta: NUM,
      cell: ({ row }) =>
        row.original.cacheRatioPct === null ? "—" : fmtPct(row.original.cacheRatioPct),
    },
    ...(view === "agent"
      ? ([
          {
            id: "hours",
            header: "Hours",
            meta: NUM,
            cell: ({ row }) => fmtHours(row.original.containerHours),
          },
          {
            id: "traces",
            header: "",
            meta: NUM,
            cell: ({ row }) => (
              <div className="flex items-center justify-end">
                <FocusToggle row={row.original} />
                <TracesCell row={row.original} />
              </div>
            ),
          },
        ] satisfies Col[])
      : []),
  ];
}

const COLUMNS = { agent: columnsFor("agent"), workflow: columnsFor("workflow") };

/** Rows arrive sorted (`sortRows`); DataTable only marks the sorted column. */
function DesktopTable({
  rows,
  view,
  sort,
  prevUnavailable,
  highlightId,
  onViewTraces,
  onSelect,
}: {
  rows: AttributionRow[];
  view: "agent" | "workflow";
  sort: SortKey;
  prevUnavailable: boolean;
  highlightId?: string | undefined;
  onViewTraces: (id: string) => void;
  onSelect?: ((id: string) => void) | undefined;
}) {
  const highlightRef = useScrollToHighlight<HTMLTableRowElement>(highlightId);
  return (
    <CellCtx value={{ prevUnavailable, onViewTraces, onSelect, highlightId }}>
      <DataTable
        className="hidden md:block"
        label="Who is driving cost"
        columns={COLUMNS[view]}
        data={rows}
        getRowId={(r) => r.id}
        sorting={[{ id: sort, desc: sort !== "name" }]}
        rowProps={(r) => (r.id === highlightId ? { ref: highlightRef, className: HIGHLIGHT } : {})}
      />
    </CellCtx>
  );
}

/** F5 → F3 cross-highlight: the highlighted row scrolls into view (both layouts). */
function useScrollToHighlight<T extends HTMLElement>(highlightId: string | undefined) {
  const ref = useRef<T | null>(null);
  useEffect(() => {
    // Native smooth scrolling isn't covered by MotionConfig reducedMotion="user".
    // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- jsdom has no scrollIntoView (the lab's optional call)
    ref.current?.scrollIntoView?.({
      block: "nearest",
      behavior: prefersReducedMotion() ? "auto" : "smooth",
    });
  }, [highlightId]);
  return ref;
}

function MobileCards({
  rows,
  view,
  prevUnavailable,
  highlightId,
  onViewTraces,
  onSelect,
}: {
  rows: AttributionRow[];
  view: "agent" | "workflow";
  prevUnavailable: boolean;
  highlightId?: string | undefined;
  onViewTraces: (id: string) => void;
  onSelect?: ((id: string) => void) | undefined;
}) {
  const highlightRef = useScrollToHighlight<HTMLLIElement>(highlightId);
  return (
    <ul className="flex flex-col gap-2 md:hidden">
      {rows.map((r) => (
        <Card
          key={r.id}
          render={<li ref={r.id === highlightId ? highlightRef : undefined} />}
          className={cn("gap-0 rounded-md p-3 shadow-none", r.id === highlightId && HIGHLIGHT)}
        >
          <div className="flex items-center justify-between gap-2">
            <span className="truncate font-medium">{r.name}</span>
            <span className="tabular-nums">{fmtMoney(r.cost)}</span>
          </div>
          <div className="mt-1 flex items-center justify-between text-xs text-muted-foreground">
            <span>{fmtPct(r.sharePct)} of spend</span>
            <RowDelta row={r} prevUnavailable={prevUnavailable} />
          </div>
          {view === "agent" ? (
            <div className="mt-1 flex gap-4">
              <Button
                variant="link"
                size="sm"
                className="h-auto min-h-11 w-fit p-0 text-xs"
                onClick={() => {
                  onViewTraces(r.id);
                }}
              >
                View traces
              </Button>
              {onSelect ? (
                <Button
                  variant="link"
                  size="sm"
                  className="h-auto min-h-11 w-fit p-0 text-xs"
                  aria-pressed={r.id === highlightId}
                  onClick={() => {
                    onSelect(r.id === highlightId ? "" : r.id);
                  }}
                >
                  {r.id === highlightId ? "In focus" : "Focus"}
                </Button>
              ) : null}
            </div>
          ) : null}
        </Card>
      ))}
    </ul>
  );
}

function RowDelta({ row, prevUnavailable }: { row: AttributionRow; prevUnavailable: boolean }) {
  if (prevUnavailable || row.deltaKind === "unavailable")
    return <Delta changePct={null} polarity={POLARITY.spend} unavailable />;
  if (row.deltaKind === "new")
    return (
      <Badge variant="secondary" className="text-2xs">
        new
      </Badge>
    );
  if (row.deltaKind === "no-spend" && row.deltaPct === null)
    return <span className="text-xs text-muted-foreground">—</span>;
  return <Delta changePct={row.deltaPct} polarity={POLARITY.spend} />;
}
