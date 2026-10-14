"use client";

import { ArrowDownRightIcon, ArrowUpRightIcon, MinusIcon } from "lucide-react";
import { cn } from "../lib/cn";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "../primitives/card";
import { Emphasis } from "../motion/recipes";
import { Skeleton } from "../primitives/skeleton";

/**
 * MetricGroup: KPI values with units and a comparison period (contract §8
 * "metric group with units and comparison period"). Rules it encodes:
 *  - the value is text, rendered immediately; a change gets a brief colour
 *    emphasis and never a count-up (§8.3 KPI / financial values);
 *  - zero, unknown and unpriced are three different things and look different;
 *  - formatting is the caller's (locale/currency come from the backend, never
 *    hard-coded here); this component only lays out what it is given;
 *  - a comparison states its period; a delta without a period is not shown.
 */
export type MetricValue =
  | { kind: "value"; text: string }
  | { kind: "zero"; text: string }
  | { kind: "unknown" }
  | { kind: "unpriced" };

export interface MetricComparison {
  /** Signed, already formatted (e.g. "+12.4%", "−$310"). */
  readonly deltaText: string;
  readonly direction: "up" | "down" | "flat";
  /** e.g. "vs previous 7 days". Required: a delta without its period is meaningless. */
  readonly period: string;
  /** Whether "up" is good for this metric (cost: no; revenue: yes). */
  readonly upIsGood?: boolean;
}

export interface Metric {
  readonly id: string;
  readonly label: string;
  readonly value: MetricValue;
  readonly unit?: string;
  readonly comparison?: MetricComparison;
  readonly description?: string;
}

export interface MetricGroupProps {
  readonly metrics: readonly Metric[];
  readonly loading?: boolean;
  readonly className?: string;
  /** Column count hint; the grid still wraps at narrow widths. */
  readonly columns?: 2 | 3 | 4;
}

function ValueText({ v, unit }: { v: MetricValue; unit?: string | undefined }) {
  switch (v.kind) {
    case "value":
      return (
        <>
          <span className="tabular-nums">{v.text}</span>
          {unit ? (
            <span className="ml-1 text-sm font-normal text-muted-foreground">{unit}</span>
          ) : null}
        </>
      );
    case "zero":
      return (
        <>
          <span className="tabular-nums text-muted-foreground">{v.text}</span>
          {unit ? (
            <span className="ml-1 text-sm font-normal text-muted-foreground">{unit}</span>
          ) : null}
        </>
      );
    case "unknown":
      return (
        <span className="text-muted-foreground" aria-label="unknown">
          —
        </span>
      );
    case "unpriced":
      return <span className="text-sm font-normal text-warning-foreground">unpriced</span>;
  }
}

function changeKeyOf(v: MetricValue): string {
  return v.kind === "value" || v.kind === "zero" ? `${v.kind}:${v.text}` : v.kind;
}

export function MetricGroup({
  metrics,
  loading = false,
  className,
  columns = 4,
}: MetricGroupProps) {
  return (
    <div
      role="group"
      aria-label="Key metrics"
      className={cn(
        "grid gap-3",
        columns === 2
          ? "sm:grid-cols-2"
          : columns === 3
            ? "sm:grid-cols-3"
            : "sm:grid-cols-2 lg:grid-cols-4",
        className,
      )}
    >
      {metrics.map((m) => {
        const c = m.comparison;
        const good = c
          ? c.direction === "flat"
            ? null
            : (c.direction === "up") === (c.upIsGood ?? true)
          : null;
        return (
          <Card key={m.id} size="sm" data-metric={m.id}>
            <CardHeader>
              <CardDescription>{m.label}</CardDescription>
              <CardTitle className="text-2xl">
                {loading ? (
                  // `aria-label` on a plain div is a prohibited ARIA attribute (axe, WCAG 4.1.2);
                  // a status role makes the pending value announceable instead of invalid.
                  <Skeleton
                    className="h-7 w-24"
                    role="status"
                    aria-busy="true"
                    aria-label="loading"
                  />
                ) : (
                  <Emphasis changeKey={changeKeyOf(m.value)}>
                    <ValueText v={m.value} unit={m.unit} />
                  </Emphasis>
                )}
              </CardTitle>
            </CardHeader>
            {c || m.description ? (
              <CardContent className="text-xs text-muted-foreground">
                {c ? (
                  <span
                    className={cn(
                      "inline-flex items-center gap-1",
                      good === true && "text-success-foreground",
                      good === false && "text-destructive",
                    )}
                  >
                    {c.direction === "up" ? (
                      <ArrowUpRightIcon className="size-3.5" aria-hidden />
                    ) : c.direction === "down" ? (
                      <ArrowDownRightIcon className="size-3.5" aria-hidden />
                    ) : (
                      <MinusIcon className="size-3.5" aria-hidden />
                    )}
                    <span className="tabular-nums">{c.deltaText}</span>
                    <span className="text-muted-foreground">{c.period}</span>
                  </span>
                ) : null}
                {m.description ? <p className="mt-1">{m.description}</p> : null}
              </CardContent>
            ) : null}
          </Card>
        );
      })}
    </div>
  );
}
