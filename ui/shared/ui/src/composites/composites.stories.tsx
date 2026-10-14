import type { Meta, StoryObj } from "@storybook/react-vite";
import { useState } from "react";
import { createGroupedFilters, type GroupedFilterState } from "@nasiko/data-access";
import type { ActivityEvent, ApprovalRequest } from "@nasiko/contracts";
import { Button } from "../primitives/button";
import { ActivityTimeline } from "./activity-timeline";
import { ApprovalCard } from "./approval-card";
import { ChartTablePair } from "./chart-table-pair";
import { FilterBar, type FilterValues } from "./filter-bar";
import { MetricGroup } from "./metric-group";
import { DeniedState, EmptyState, ErrorState, LoadingState, PartialDataNotice } from "./states";

/**
 * Composites are *proposed Nasiko capabilities* composed from shadcn primitives
 * and the admitted extensions — not upstream components (Phase 3 exit:
 * "proposed composites clearly distinguished from upstream components").
 * All data here is synthetic.
 */
const meta = { title: "Composites (Nasiko)/Overview" } satisfies Meta;
export default meta;

export const Metrics: StoryObj = {
  render: function MetricsDemo() {
    const [tick, setTick] = useState(0);
    const spend = (1204.5 + tick * 13.25).toFixed(2);
    return (
      <div className="flex flex-col gap-3">
        <MetricGroup
          metrics={[
            {
              id: "spend",
              label: "Spend (30d)",
              value: { kind: "value", text: `$${spend}` },
              comparison: {
                deltaText: "+4.1%",
                direction: "up",
                period: "vs previous 30 days",
                upIsGood: false,
              },
            },
            {
              id: "req",
              label: "Requests",
              value: { kind: "value", text: "48,120" },
              comparison: { deltaText: "−2.0%", direction: "down", period: "vs previous 30 days" },
            },
            {
              id: "err",
              label: "Errors",
              value: { kind: "zero", text: "0" },
              comparison: { deltaText: "0", direction: "flat", period: "vs previous 30 days" },
            },
            {
              id: "lat",
              label: "p95 latency",
              value: { kind: "unknown" },
              description: "No samples in range",
            },
            {
              id: "mx",
              label: "Model X cost",
              value: { kind: "unpriced" },
              description: "No price configured",
            },
          ]}
        />
        <Button
          size="sm"
          variant="outline"
          className="self-start"
          onClick={() => {
            setTick((t) => t + 1);
          }}
        >
          Simulate refetch
        </Button>
      </div>
    );
  },
};

export const MetricsLoading: StoryObj = {
  render: () => (
    <MetricGroup
      loading
      metrics={[
        { id: "a", label: "Spend", value: { kind: "unknown" } },
        { id: "b", label: "Requests", value: { kind: "unknown" } },
      ]}
    />
  ),
};

export const Filters: StoryObj = {
  render: function FiltersDemo() {
    const [state, setState] = useState<GroupedFilterState<FilterValues>>(() =>
      createGroupedFilters<FilterValues>({ q: "", agent: "", range: "7d" }),
    );
    return (
      <div className="flex flex-col gap-3">
        <FilterBar
          state={state}
          onChange={setState}
          fields={[
            { kind: "text", key: "q", label: "Search", placeholder: "Agent or model" },
            {
              kind: "select",
              key: "agent",
              label: "Agent",
              placeholder: "All",
              options: [
                { value: "alpha", label: "alpha-agent" },
                { value: "beta", label: "beta-agent" },
              ],
            },
            {
              kind: "preset-range",
              key: "range",
              label: "Range",
              presets: [
                { value: "24h", label: "Last 24 hours" },
                { value: "7d", label: "Last 7 days" },
                { value: "30d", label: "Last 30 days" },
              ],
            },
          ]}
        />
        <pre className="rounded bg-muted p-2 text-xs">
          {JSON.stringify(
            { applied: state.applied, appliedVersion: state.appliedVersion, draft: state.draft },
            null,
            2,
          )}
        </pre>
      </div>
    );
  },
};

const approval: ApprovalRequest = {
  id: "0f5ae0a2-0c1e-4b9e-9c2a-1d1b3c4d5e6f",
  actionId: "act_scale_42",
  title: "Scale alpha-agent to 4 replicas",
  summary:
    "The agent proposes scaling the deployment to absorb the queue backlog observed in the last 15 minutes.",
  tool: "k8s.scale",
  params: [
    { name: "deployment", kind: "text", value: "alpha-agent" },
    { name: "replicas", kind: "number", value: "4" },
    { name: "estimated cost", kind: "money", value: "$12.40", unit: "/day" },
    { name: "api token", kind: "secret", value: "••••••••" },
    { name: "runbook", kind: "link", value: "https://docs.nasiko.example/runbooks/scale" },
  ],
  risk: "medium",
  state: "pending",
  requestedAt: "2026-10-06T10:00:00Z",
  expiresAt: "2026-10-06T10:15:00Z",
  decidableBy: ["00000000-0000-4000-8000-000000000001"],
  links: [{ href: "/workspaces/ws-a/agents/alpha", label: "Agent" }],
};

export const Approval: StoryObj = {
  render: function ApprovalDemo() {
    const [state, setState] = useState<ApprovalRequest["state"]>("pending");
    return (
      <ApprovalCard
        request={{ ...approval, state }}
        currentUserId="00000000-0000-4000-8000-000000000001"
        onDecide={(d) => {
          setState(d.decision === "approve" ? "approved" : "rejected");
        }}
      />
    );
  },
};

export const ApprovalStates: StoryObj = {
  render: () => (
    <div className="grid gap-4 md:grid-cols-2">
      {(["pending", "approved", "rejected", "expired", "executing", "failed"] as const).map((s) => (
        <ApprovalCard key={s} request={{ ...approval, state: s }} currentUserId="someone-else" />
      ))}
    </div>
  ),
};

const events: ActivityEvent[] = [
  {
    id: "1",
    kind: "message",
    at: "2026-10-06T10:00:00Z",
    text: "Investigating the queue backlog on alpha-agent.",
    actor: "agent",
  },
  {
    id: "2",
    kind: "tool.start",
    at: "2026-10-06T10:00:02Z",
    text: "usage.query(range=15m, agent=alpha)",
    actor: "tool",
    status: "running",
    correlationId: "c1",
  },
  {
    id: "3",
    kind: "tool.result",
    at: "2026-10-06T10:00:04Z",
    text: "1,240 queued requests; p95 latency 3.1 s.",
    actor: "tool",
    status: "ok",
    correlationId: "c1",
  },
  {
    id: "4",
    kind: "approval.requested",
    at: "2026-10-06T10:00:05Z",
    text: "Requested approval to scale to 4 replicas.",
    actor: "agent",
    links: [{ href: "/approvals/act_scale_42", label: "Approval" }],
  },
];

export const Timeline: StoryObj = {
  render: function TimelineDemo() {
    const [list, setList] = useState(events);
    return (
      <div className="flex flex-col gap-3">
        <ActivityTimeline events={list} formatTime={(iso) => iso.slice(11, 19)} />
        <Button
          size="sm"
          variant="outline"
          className="self-start"
          onClick={() => {
            setList((l) => [
              ...l,
              {
                id: `n${l.length}`,
                kind: "status",
                at: new Date().toISOString(),
                text: `Backend event ${l.length + 1} arrived.`,
                actor: "system",
                status: "ok",
              },
            ]);
          }}
        >
          Append a backend event
        </Button>
      </div>
    );
  },
};

export const States: StoryObj = {
  render: () => (
    <div className="grid gap-4 md:grid-cols-2">
      <LoadingState label="Loading spend" height={96} />
      <EmptyState
        title="No agents yet"
        description="Create an agent to see usage here."
        action={<Button size="sm">Create agent</Button>}
      />
      <ErrorState
        title="Couldn't load usage"
        description="The usage service returned an error."
        onRetry={() => undefined}
        requestId="req_01HZX"
      />
      <DeniedState description="Ask a workspace admin for the finops:read role." />
      <PartialDataNotice>Showing 28 of 30 days — two days have no samples.</PartialDataNotice>
    </div>
  ),
};

export const ChartTable: StoryObj = {
  render: () => (
    <ChartTablePair
      title="Spend by day"
      querySummary="usage.spend_timeseries · last 14 days · 14 rows"
      chart={
        <div className="flex h-40 items-center justify-center rounded bg-muted text-sm text-muted-foreground">
          chart slot (lazy — see Charts stories)
        </div>
      }
      table={
        <div className="rounded bg-muted p-3 text-sm text-muted-foreground">
          table slot (same rows)
        </div>
      }
      notice={<PartialDataNotice>Yesterday is still being aggregated.</PartialDataNotice>}
    />
  ),
};
