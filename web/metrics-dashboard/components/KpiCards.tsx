"use client";

import {
  formatCostUsd,
  formatCount,
  formatLatencyMs,
  formatPercent,
  formatQualityScore,
  formatSteps,
} from "@/lib/format";
import type { AgentMetricsResponse } from "@/lib/types";
import { CostSplit } from "@/components/CostSplit";
import { FeedbackBar } from "@/components/FeedbackBar";
import { LatencySparkline } from "@/components/LatencySparkline";

type Props = {
  metrics: AgentMetricsResponse | null;
  loading?: boolean;
  noData?: boolean;
};

type Tone = "neutral" | "good" | "bad";

type Item = {
  key: string;
  label: string;
  value: string;
  hint: string;
  tone: Tone;
  extra?: React.ReactNode;
};

/** Core metrics at a glance — plain language for stakeholders. */
export function KpiCards({ metrics, loading, noData }: Props) {
  const showPlaceholders = loading || noData || metrics == null;
  const placeholder = loading ? "…" : "—";

  const latencyValue = showPlaceholders
    ? placeholder
    : formatLatencyMs(metrics?.avg_latency_ms);

  const quality = metrics?.source?.quality;
  const orchestration = metrics?.source?.orchestration;
  const costBreakdown = metrics?.source?.cost_breakdown;

  const items: Item[] = [
    {
      key: "latency",
      label: "Typical response time",
      value: latencyValue,
      hint: "How long users wait for an answer",
      tone: "neutral",
      extra: !showPlaceholders ? (
        <LatencySparkline
          series={metrics?.series_24h}
          ariaLabel={`24-hour response time trend, typical ${latencyValue}`}
        />
      ) : null,
    },
    {
      key: "success",
      label: "Successful requests",
      value: showPlaceholders ? placeholder : formatCount(metrics?.success),
      hint: "Completed without errors · last 24 hours",
      tone: "good",
    },
    {
      key: "errors",
      label: "Failed requests",
      value: showPlaceholders ? placeholder : formatCount(metrics?.error),
      hint: "Something went wrong for the user",
      tone: (metrics?.error ?? 0) > 0 ? "bad" : "neutral",
    },
    {
      key: "uptime",
      label: "Availability",
      value: showPlaceholders ? placeholder : formatPercent(metrics?.uptime_pct),
      hint: "Agent was up and reachable",
      tone: "neutral",
    },
    {
      key: "cost",
      label: "Estimated cost",
      value: showPlaceholders ? placeholder : formatCostUsd(metrics?.source?.cost_usd),
      hint: noData ? "No usage data yet" : "AI usage cost · last 24 hours",
      tone: "neutral",
      extra: !showPlaceholders && costBreakdown ? (
        <CostSplit breakdown={costBreakdown} />
      ) : null,
    },
  ];

  // Tier B — render the quality / feedback card only when annotations exist.
  if (!showPlaceholders && quality && (quality.sample_count > 0 || quality.top_labels.length > 0)) {
    items.push({
      key: "quality",
      label: "User feedback",
      value:
        quality.sample_count > 0
          ? formatQualityScore(quality.mean_score)
          : quality.top_labels[0]
            ? `${(quality.top_labels[0].fraction * 100).toFixed(0)}% ${quality.top_labels[0].label}`
            : "—",
      hint:
        quality.sample_count > 0
          ? `Mean ${quality.primary_annotation ?? "satisfaction"} · ${quality.sample_count} rated sessions`
          : "Top label across rated sessions",
      tone:
        quality.mean_score == null
          ? "neutral"
          : quality.mean_score > 0.7 || quality.mean_score >= 4
            ? "good"
            : quality.mean_score < 0.4 || quality.mean_score < 2
              ? "bad"
              : "neutral",
      extra: <FeedbackBar labels={quality.top_labels} />,
    });
  }

  // Tier B — orchestration depth (avg steps per request) — only show when we have data.
  if (
    !showPlaceholders &&
    orchestration &&
    orchestration.avg_traces_per_session != null
  ) {
    items.push({
      key: "orchestration",
      label: "Avg steps per request",
      value: formatSteps(orchestration.avg_traces_per_session),
      hint: `${orchestration.total_traces.toLocaleString()} steps across ${orchestration.total_sessions.toLocaleString()} sessions`,
      tone: "neutral",
    });
  }

  return (
    <div className="kpi-grid" role="list" aria-label="Performance at a glance">
      {items.map((item) => (
        <article
          key={item.key}
          className="kpi-card"
          role="listitem"
          data-tone={item.tone}
          aria-busy={loading || undefined}
        >
          <p className="kpi-card__label">{item.label}</p>
          <p className="kpi-card__value" aria-label={`${item.label}: ${item.value}`}>
            {item.value}
          </p>
          {item.extra ?? null}
          <p className="kpi-card__hint">{item.hint}</p>
        </article>
      ))}
    </div>
  );
}
