import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { createGroupedFilters, type GroupedFilterState } from "@nasiko/data-access";
import type { ActivityEvent, ApprovalRequest } from "@nasiko/contracts";
import { MotionProvider } from "../motion/provider";
import { ActivityTimeline } from "./activity-timeline";
import { ApprovalCard } from "./approval-card";
import { FilterBar, type FilterValues } from "./filter-bar";
import { MetricGroup } from "./metric-group";

const wrap = (ui: React.ReactElement) =>
  render(<MotionProvider osReducedOverride={true}>{ui}</MotionProvider>);

const request: ApprovalRequest = {
  id: "0f5ae0a2-0c1e-4b9e-9c2a-1d1b3c4d5e6f",
  actionId: "act_1",
  title: "Scale deployment",
  summary: "Scale agent alpha from 2 to 4 replicas.",
  tool: "k8s.scale",
  params: [
    { name: "replicas", kind: "number", value: "4" },
    { name: "token", kind: "secret", value: "••••••••" },
  ],
  risk: "medium",
  state: "pending",
  requestedAt: "2026-10-06T10:00:00Z",
  decidableBy: ["1b2c3d4e-0000-4000-8000-000000000001"],
};

describe("ApprovalCard", () => {
  it("enables decisions only for a listed decider, and sends exactly the server's ids back", async () => {
    const onDecide = vi.fn();
    wrap(
      <ApprovalCard
        request={request}
        currentUserId="1b2c3d4e-0000-4000-8000-000000000001"
        onDecide={onDecide}
      />,
    );
    const approve = screen.getByRole("button", { name: "Approve: Scale deployment" });
    expect(approve).toBeEnabled();
    await userEvent.click(approve);
    expect(onDecide).toHaveBeenCalledWith({
      id: request.id,
      actionId: "act_1",
      decision: "approve",
    });
  });
  it("disables decisions for anyone else and in every non-pending state", () => {
    const first = wrap(
      <ApprovalCard request={request} currentUserId="someone-else" onDecide={vi.fn()} />,
    );
    expect(screen.getByRole("button", { name: /Approve/ })).toBeDisabled();
    first.unmount();
    for (const state of ["approved", "rejected", "expired", "executing", "failed"] as const) {
      const { unmount } = wrap(
        <ApprovalCard
          request={{ ...request, state }}
          currentUserId="1b2c3d4e-0000-4000-8000-000000000001"
          onDecide={vi.fn()}
        />,
      );
      expect(screen.queryByRole("button", { name: /Approve/ })).toBeNull();
      unmount();
    }
  });
  it("is a labelled region whose state is announced", () => {
    wrap(<ApprovalCard request={request} currentUserId="x" />);
    const region = screen.getByRole("region", { name: "Scale deployment" });
    expect(within(region).getByText("Awaiting your decision")).toBeInTheDocument();
    expect(region).toHaveAttribute("data-approval-state", "pending");
  });
});

describe("MetricGroup", () => {
  it("renders zero, unknown and unpriced distinctly and never animates a number", () => {
    wrap(
      <MetricGroup
        metrics={[
          {
            id: "a",
            label: "Spend",
            value: { kind: "value", text: "$1,204.50" },
            comparison: {
              deltaText: "+4.1%",
              direction: "up",
              period: "vs previous 7 days",
              upIsGood: false,
            },
          },
          { id: "b", label: "Errors", value: { kind: "zero", text: "0" } },
          { id: "c", label: "Latency", value: { kind: "unknown" } },
          { id: "d", label: "Model X", value: { kind: "unpriced" } },
        ]}
      />,
    );
    expect(screen.getByText("$1,204.50")).toBeInTheDocument();
    expect(screen.getByText("0")).toBeInTheDocument();
    expect(screen.getByLabelText("unknown")).toHaveTextContent("—");
    expect(screen.getByText("unpriced")).toBeInTheDocument();
    expect(screen.getByText("vs previous 7 days")).toBeInTheDocument();
  });
});

describe("FilterBar", () => {
  const fields = [
    { kind: "text", key: "q", label: "Search" },
    {
      kind: "preset-range",
      key: "range",
      label: "Range",
      presets: [{ value: "7d", label: "7 days" }],
    },
  ] as const;
  function Harness({ onState }: { onState: (s: GroupedFilterState<FilterValues>) => void }) {
    const [state, setState] = useState(() =>
      createGroupedFilters<FilterValues>({ q: "", range: "7d" }),
    );
    return (
      <FilterBar
        fields={fields}
        state={state}
        onChange={(n) => {
          setState(n);
          onState(n);
        }}
      />
    );
  }
  it("edits the draft freely and bumps appliedVersion exactly once on Apply", async () => {
    const seen: GroupedFilterState<FilterValues>[] = [];
    wrap(<Harness onState={(s) => seen.push(s)} />);
    await userEvent.type(screen.getByLabelText("Search"), "abc");
    expect(seen.at(-1)?.draft.q).toBe("abc");
    expect(seen.every((s) => s.appliedVersion === 0)).toBe(true);
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(seen.at(-1)).toMatchObject({ appliedVersion: 1, applied: { q: "abc", range: "7d" } });
    expect(seen.filter((s) => s.appliedVersion === 1)).toHaveLength(1);
  });
});

describe("ActivityTimeline", () => {
  const ev = (i: number): ActivityEvent => ({
    id: `e${i}`,
    kind: "status",
    at: "2026-10-06T10:00:00Z",
    text: `event ${i}`,
    actor: "agent",
  });
  it("bounds what is shown and offers the rest on demand", async () => {
    const events = Array.from({ length: 30 }, (_, i) => ev(i));
    wrap(<ActivityTimeline events={events} initialVisible={5} />);
    expect(screen.getAllByRole("listitem")).toHaveLength(5);
    await userEvent.click(screen.getByRole("button", { name: "Show 25 earlier" }));
    expect(screen.getAllByRole("listitem")).toHaveLength(30);
  });
  it("renders only given events — no synthetic rows", () => {
    wrap(<ActivityTimeline events={[ev(1)]} />);
    expect(screen.getAllByRole("listitem")).toHaveLength(1);
    expect(screen.queryByText(/thinking/i)).toBeNull();
  });
});
