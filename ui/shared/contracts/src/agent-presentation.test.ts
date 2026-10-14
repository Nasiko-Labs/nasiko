import { describe, expect, it } from "vitest";
import {
  ActivityEventSchema,
  ApprovalDecisionSchema,
  ApprovalRequestSchema,
} from "./agent-presentation";

const base = {
  id: "0f5ae0a2-0c1e-4b9e-9c2a-1d1b3c4d5e6f",
  actionId: "act_1",
  title: "Scale deployment",
  summary: "Scale agent alpha from 2 to 4 replicas.",
  tool: "k8s.scale",
  params: [{ name: "replicas", kind: "number", value: "4" }],
  risk: "medium",
  state: "pending",
  requestedAt: "2026-10-06T10:00:00Z",
};

describe("ApprovalRequestSchema", () => {
  it("accepts a server-issued request", () => {
    expect(ApprovalRequestSchema.safeParse(base).success).toBe(true);
  });
  it("is strict: the UI cannot be fed fields it would be tempted to render (progress, confidence)", () => {
    expect(ApprovalRequestSchema.safeParse({ ...base, progress: 0.5 }).success).toBe(false);
    expect(ApprovalRequestSchema.safeParse({ ...base, confidence: "high" }).success).toBe(false);
  });
  it("rejects unknown states, risks and param kinds", () => {
    expect(ApprovalRequestSchema.safeParse({ ...base, state: "maybe" }).success).toBe(false);
    expect(ApprovalRequestSchema.safeParse({ ...base, risk: "critical" }).success).toBe(false);
    expect(
      ApprovalRequestSchema.safeParse({
        ...base,
        params: [{ name: "x", kind: "html", value: "<b>" }],
      }).success,
    ).toBe(false);
  });
  it("bounds params and links", () => {
    const params = Array.from({ length: 33 }, (_, i) => ({
      name: `p${i}`,
      kind: "text",
      value: "v",
    }));
    expect(ApprovalRequestSchema.safeParse({ ...base, params }).success).toBe(false);
  });
  it("a decision carries only approve/reject plus an optional bounded reason", () => {
    expect(
      ApprovalDecisionSchema.safeParse({ id: base.id, actionId: "act_1", decision: "approve" })
        .success,
    ).toBe(true);
    expect(
      ApprovalDecisionSchema.safeParse({ id: base.id, actionId: "act_1", decision: "execute" })
        .success,
    ).toBe(false);
    expect(
      ApprovalDecisionSchema.safeParse({
        id: base.id,
        actionId: "act_1",
        decision: "reject",
        reason: "x".repeat(501),
      }).success,
    ).toBe(false);
  });
});

describe("ActivityEventSchema", () => {
  it("accepts backend events and rejects client-side inventions", () => {
    const ev = {
      id: "e1",
      kind: "tool.start",
      at: "2026-10-06T10:00:00Z",
      text: "Querying usage",
      actor: "tool",
      status: "running",
      correlationId: "c1",
    };
    expect(ActivityEventSchema.safeParse(ev).success).toBe(true);
    expect(ActivityEventSchema.safeParse({ ...ev, percent: 40 }).success).toBe(false);
    expect(ActivityEventSchema.safeParse({ ...ev, kind: "thinking" }).success).toBe(false);
  });
});
