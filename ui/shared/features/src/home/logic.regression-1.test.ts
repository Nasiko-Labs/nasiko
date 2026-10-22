// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/logic.regression-1.test.ts. Local edits: Prettier formatting.
/**
 * The home's sentence must agree with the strip when calls went unpriced: the strip shows a floor ("≥"), so the
 * sentence can't state the same sum as exact, and nothing priced is not nothing spent.
 */
// Regression: ISSUE-001 — the sentence said "spent $144.46" while the strip said "≥ $144.46, at least this much"
// Found by /qa on 2026-10-11
// Report: .gstack/qa-reports/qa-report-localhost-2026-10-11.md
import { describe, expect, it } from "vitest";
import { homeNarrative } from "../narrative/home";

const base = { scope: "yours" as const, agents: 15, harnesses: 4, days: 30, needing: 3 };

describe("the sentence with unpriced calls", () => {
  it("reads the spend as a floor", () => {
    expect(homeNarrative({ ...base, spend: 144.46, floor: true })).toBe(
      "Your 15 agents and 4 harnesses spent at least $144.46 in the last 30 days; 3 need you.",
    );
  });

  it("leaves the amount out when nothing was priced, rather than say nothing was spent", () => {
    expect(homeNarrative({ ...base, spend: 0, floor: true })).toBe(
      "Your 15 agents and 4 harnesses; 3 need you.",
    );
    expect(homeNarrative({ ...base, spend: 0, floor: true, needing: 0 })).toBeNull();
  });

  it("keeps the exact wording when every call was priced", () => {
    expect(homeNarrative({ ...base, spend: 144.46 })).toBe(
      "Your 15 agents and 4 harnesses spent $144.46 in the last 30 days; 3 need you.",
    );
    expect(homeNarrative({ ...base, spend: 0, needing: 0 })).toBe(
      "Your 15 agents and 4 harnesses spent nothing in the last 30 days.",
    );
  });
});
