"use client";

import { BarChart3Icon, Rows3Icon } from "lucide-react";
import { useId, useState, type ReactNode } from "react";
import { cn } from "../lib/cn";
import { ToggleGroup, ToggleGroupItem } from "../primitives/toggle-group";

/**
 * ChartTablePair: one query, two views (contract §8 "chart/table pair sharing
 * a query"; "detailed tables provide drill-down rather than duplicating every
 * chart"). The caller renders both views from the *same* rows and passes them
 * as slots, so this composite can never show a chart and a table that
 * disagree. The chart slot is expected to come through a lazy boundary
 * (`@nasiko/ui/charts` is a separate entry); this file imports neither charts
 * nor the table, so it is safe in the root bundle.
 *
 * Switching views is instant; nothing crossfades (§8.3 charts/tables).
 */
export interface ChartTablePairProps {
  readonly title: string;
  /** e.g. "Spend by day, last 30 days · 1,240 rows" — the shared query, stated once. */
  readonly querySummary: string;
  readonly chart: ReactNode;
  readonly table: ReactNode;
  readonly defaultView?: "chart" | "table" | "both";
  readonly className?: string;
  /** Rendered above both views, e.g. a PartialDataNotice. */
  readonly notice?: ReactNode;
}

export function ChartTablePair({
  title,
  querySummary,
  chart,
  table,
  defaultView = "chart",
  className,
  notice,
}: ChartTablePairProps) {
  const id = useId();
  const [view, setView] = useState<"chart" | "table" | "both">(defaultView);
  return (
    <section
      aria-labelledby={`${id}-t`}
      className={cn(
        "flex flex-col gap-3 rounded-xl bg-card p-4 ring-1 ring-foreground/10",
        className,
      )}
      data-view={view}
    >
      <header className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <h3 id={`${id}-t`} className="cn-font-heading text-base font-medium">
            {title}
          </h3>
          <p className="text-xs text-muted-foreground">{querySummary}</p>
        </div>
        <ToggleGroup
          value={[view]}
          onValueChange={(v) => {
            const next = v[0];
            if (next === "chart" || next === "table" || next === "both") setView(next);
          }}
          aria-label="View"
          variant="outline"
          size="sm"
        >
          <ToggleGroupItem value="chart" aria-label="Chart">
            <BarChart3Icon aria-hidden />
          </ToggleGroupItem>
          <ToggleGroupItem value="table" aria-label="Table">
            <Rows3Icon aria-hidden />
          </ToggleGroupItem>
          <ToggleGroupItem value="both" aria-label="Chart and table">
            Both
          </ToggleGroupItem>
        </ToggleGroup>
      </header>
      {notice}
      {view !== "table" ? <div data-slot="chart-view">{chart}</div> : null}
      {view !== "chart" ? <div data-slot="table-view">{table}</div> : null}
    </section>
  );
}
