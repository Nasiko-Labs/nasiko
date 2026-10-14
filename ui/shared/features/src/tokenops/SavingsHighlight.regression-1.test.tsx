// Ported from nasiko-ui-lab@1b99bdda common/src/features/tokenops/SavingsHighlight.regression-1.test.tsx. Local edits: `renderApp` -> `renderTokenops`; Prettier formatting.
// Regression: ISSUE-002 — TokenOps' savings line lower-cased the whole program label, so the codename read "(caveman)"
// Found by /qa on 2026-10-05
// Report: .gstack/qa-reports/qa-report-localhost-2026-10-05.md
import { screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { setupPinnedSeed } from "@nasiko/testing/pinned-seed";
import { renderTokenops } from "./test-utils";

setupPinnedSeed();

describe("TokenOps savings line (ISSUE-002)", () => {
  it("lower-cases only the first letter, so the team name keeps its capital", async () => {
    renderTokenops("/tokenops");
    expect(
      await screen.findByText(/— mostly smaller prompts \(Caveman\)/, {}, { timeout: 12_000 }),
    ).toBeInTheDocument();
  });
});
