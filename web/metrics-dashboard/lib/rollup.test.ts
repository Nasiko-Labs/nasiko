import { describe, expect, it } from "vitest";
import {
  aggregateOrchestrationFromSessions,
  aggregateQualityFromSessions,
  buildAgentMetrics,
  buildCostBreakdown,
  buildSeries24h,
  isSessionError,
  rollupSessionCounts,
} from "@/lib/rollup";
import type {
  NasikoAgentProjectStats,
  NasikoObservabilitySession,
} from "@/lib/types";

const NOW = Date.parse("2026-05-16T14:30:00.000Z");

describe("isSessionError", () => {
  it("detects error text in last_output", () => {
    expect(
      isSessionError({
        last_output: { value: "Sorry, an error occurred while processing" },
      }),
    ).toBe(true);
  });

  it("treats normal output as success", () => {
    expect(
      isSessionError({
        last_output: { value: "Hola — translation complete." },
      }),
    ).toBe(false);
  });
});

describe("rollupSessionCounts", () => {
  it("sums traces into success and error buckets", () => {
    const sessions: NasikoObservabilitySession[] = [
      { num_traces: 2, last_output: { value: "ok" } },
      {
        num_traces: 1,
        last_output: { value: "Error code: 401 - Missing Authentication header" },
      },
    ];
    expect(rollupSessionCounts(sessions)).toEqual({ success: 2, error: 1 });
  });
});

describe("buildSeries24h", () => {
  it("fills hourly buckets for session activity", () => {
    const sessions: NasikoObservabilitySession[] = [
      {
        start_time: "2026-05-16T13:15:00.000Z",
        num_traces: 2,
        trace_latency_ms_p50: 1000,
        last_output: { value: "done" },
      },
      {
        start_time: "2026-05-16T13:45:00.000Z",
        num_traces: 1,
        trace_latency_ms_p50: 2000,
        last_output: { value: "Error: failed" },
      },
    ];

    const series = buildSeries24h(sessions, 24, NOW);
    expect(series).toHaveLength(24);

    const active = series.filter((p) => p.success != null || p.error != null);
    expect(active.length).toBeGreaterThanOrEqual(1);

    const hour13 = series.find((p) => p.ts === "2026-05-16T13:00:00.000Z");
    expect(hour13?.success).toBe(2);
    expect(hour13?.error).toBe(1);
    expect(hour13?.latency_ms).toBe(1500);
  });
});

describe("buildAgentMetrics", () => {
  it("merges stats and sessions into dashboard shape", () => {
    const project: NasikoAgentProjectStats = {
      trace_count: 4,
      latency_ms_p50: 3595,
      latency_ms_p99: 4977.8,
    };
    const sessions: NasikoObservabilitySession[] = [
      {
        agent_id: "a2a-translator",
        start_time: "2026-05-16T13:00:00.000Z",
        num_traces: 3,
        trace_latency_ms_p50: 3000,
        last_output: { value: "ok" },
      },
      {
        agent_id: "a2a-translator",
        start_time: "2026-05-16T13:30:00.000Z",
        num_traces: 1,
        trace_latency_ms_p50: 4000,
        last_output: { value: "Error code: 500" },
      },
    ];

    const metrics = buildAgentMetrics({
      agentId: "a2a-translator",
      window: "24h",
      hours: 24,
      project,
      sessions,
      uptimePct: 100,
      uptimeMeta: {
        successful_checks: 1,
        recorded_checks: 1,
        expected_checks_24h: 288,
        poll_interval_minutes: 5,
        current_health: true,
      },
      now: NOW,
    });

    expect(metrics.agent).toBe("a2a-translator");
    expect(metrics.success).toBe(3);
    expect(metrics.error).toBe(1);
    expect(metrics.avg_latency_ms).toBe(3500);
    expect(metrics.uptime_pct).toBe(100);
    expect(metrics.series_24h).toHaveLength(24);
    expect(metrics.source?.trace_count).toBe(4);
  });
});

describe("aggregateQualityFromSessions (Tier B)", () => {
  it("returns null when no annotations exist", () => {
    const sessions: NasikoObservabilitySession[] = [
      { num_traces: 1, last_output: { value: "ok" } },
    ];
    expect(aggregateQualityFromSessions(sessions)).toBeNull();
  });

  it("averages mean_score across annotated sessions", () => {
    const sessions: NasikoObservabilitySession[] = [
      {
        num_traces: 1,
        session_annotation_summaries: [
          { name: "satisfaction", mean_score: 0.8, label_fractions: [] },
        ],
      },
      {
        num_traces: 1,
        session_annotation_summaries: [
          { name: "satisfaction", mean_score: 0.6, label_fractions: [] },
        ],
      },
    ];
    const quality = aggregateQualityFromSessions(sessions);
    expect(quality?.mean_score).toBeCloseTo(0.7);
    expect(quality?.sample_count).toBe(2);
    expect(quality?.primary_annotation).toBe("satisfaction");
  });

  it("aggregates top label fractions sorted descending", () => {
    const sessions: NasikoObservabilitySession[] = [
      {
        num_traces: 1,
        session_annotation_summaries: [
          {
            name: "feedback",
            mean_score: null,
            label_fractions: [
              { label: "thumbs_up", fraction: 0.9 },
              { label: "thumbs_down", fraction: 0.1 },
            ],
          },
        ],
      },
      {
        num_traces: 1,
        session_annotation_summaries: [
          {
            name: "feedback",
            mean_score: null,
            label_fractions: [
              { label: "thumbs_up", fraction: 0.7 },
              { label: "thumbs_down", fraction: 0.3 },
            ],
          },
        ],
      },
    ];
    const quality = aggregateQualityFromSessions(sessions);
    expect(quality?.top_labels[0]).toEqual({
      label: "thumbs_up",
      fraction: 0.8,
    });
    expect(quality?.top_labels[1].label).toBe("thumbs_down");
  });
});

describe("aggregateOrchestrationFromSessions (Tier B)", () => {
  it("returns null avg when no traces in window", () => {
    const sessions: NasikoObservabilitySession[] = [{ num_traces: 0 }];
    const out = aggregateOrchestrationFromSessions(sessions);
    expect(out.avg_traces_per_session).toBeNull();
    expect(out.total_sessions).toBe(0);
  });

  it("averages num_traces over sessions that have traces", () => {
    const sessions: NasikoObservabilitySession[] = [
      { num_traces: 4 },
      { num_traces: 2 },
      { num_traces: 0 }, // ignored
    ];
    const out = aggregateOrchestrationFromSessions(sessions);
    expect(out.avg_traces_per_session).toBe(3);
    expect(out.total_sessions).toBe(2);
    expect(out.total_traces).toBe(6);
  });
});

describe("buildCostBreakdown (Tier B)", () => {
  it("returns null when no cost fields are present", () => {
    expect(buildCostBreakdown({ cost_summary: {} })).toBeNull();
  });

  it("preserves prompt / completion / total", () => {
    const out = buildCostBreakdown({
      cost_summary: {
        prompt: { cost: 0.012 },
        completion: { cost: 0.005 },
        total: { cost: 0.017 },
      },
    });
    expect(out).toEqual({
      prompt_usd: 0.012,
      completion_usd: 0.005,
      total_usd: 0.017,
    });
  });

  it("infers total from prompt + completion when missing", () => {
    const out = buildCostBreakdown({
      cost_summary: { prompt: { cost: 0.01 }, completion: { cost: 0.02 } },
    });
    expect(out?.total_usd).toBeCloseTo(0.03);
  });
});

describe("buildAgentMetrics — Tier B fields", () => {
  it("includes quality / orchestration / cost_breakdown when data exists", () => {
    const project: NasikoAgentProjectStats = {
      trace_count: 2,
      latency_ms_p50: 1000,
      latency_ms_p99: 2000,
      cost_summary: {
        prompt: { cost: 0.01 },
        completion: { cost: 0.005 },
        total: { cost: 0.015 },
      },
    };
    const sessions: NasikoObservabilitySession[] = [
      {
        agent_id: "a2a-translator",
        start_time: "2026-05-16T13:00:00.000Z",
        num_traces: 3,
        trace_latency_ms_p50: 1100,
        last_output: { value: "ok" },
        session_annotation_summaries: [
          {
            name: "satisfaction",
            mean_score: 0.9,
            label_fractions: [{ label: "thumbs_up", fraction: 1.0 }],
          },
        ],
      },
    ];

    const metrics = buildAgentMetrics({
      agentId: "a2a-translator",
      window: "24h",
      hours: 24,
      project,
      sessions,
      uptimePct: 100,
      now: NOW,
    });

    expect(metrics.source?.quality?.mean_score).toBeCloseTo(0.9);
    expect(metrics.source?.quality?.top_labels[0].label).toBe("thumbs_up");
    expect(metrics.source?.orchestration?.avg_traces_per_session).toBe(3);
    expect(metrics.source?.cost_breakdown?.prompt_usd).toBeCloseTo(0.01);
    expect(metrics.source?.cost_breakdown?.completion_usd).toBeCloseTo(0.005);
  });

  it("omits Tier B blocks gracefully when no annotations or sessions", () => {
    const metrics = buildAgentMetrics({
      agentId: "a2a-translator",
      window: "24h",
      hours: 24,
      project: { trace_count: 0 },
      sessions: [],
      uptimePct: null,
      now: NOW,
    });
    expect(metrics.source?.quality).toBeUndefined();
    expect(metrics.source?.orchestration).toBeUndefined();
    expect(metrics.source?.cost_breakdown).toBeUndefined();
  });
});
