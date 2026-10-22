// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/HomePage.regression-3.test.tsx. Local edits: Prettier formatting.
// Regression: ISSUE-003 — Fleet's owner group subtotals read "$143.69" while the strip read "≥ $143.69" (unpriced calls)
// Found by /qa on 2026-10-11
// Report: .gstack/qa-reports/qa-report-localhost-2026-10-11-pr2b.md
import { screen, waitFor, within } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { describe, expect, it } from "vitest";
import { setupPinnedSeed } from "@nasiko/testing/pinned-seed";
import { renderApp } from "@nasiko/app-shell/testing/render-app";
import { server } from "@nasiko/testing/pinned-seed";
import { copy } from "./copy";

setupPinnedSeed();

describe("owner group subtotals with unpriced calls (ISSUE-003)", () => {
  it("read as a floor, like the strip’s spend", async () => {
    // A small fleet (the full seed virtualises past 100 rows, which jsdom can't lay out): the viewer's first three.
    const page = (await (await fetch("/api/agents?limit=100&offset=0")).json()) as unknown[];
    server.use(
      http.get("/api/agents", ({ request }) =>
        HttpResponse.json(
          Number(new URL(request.url).searchParams.get("offset") ?? 0) ? [] : page.slice(0, 3),
        ),
      ),
    );
    renderApp("/?scope=fleet");
    const strip = await screen.findByRole("region", { name: copy.strip.label });
    // The pinned seed has unpriced calls: the strip says so first.
    await waitFor(() => expect(within(strip).getByText(copy.strip.unpriced)).toBeInTheDocument());
    const t = await screen.findByRole("table", { name: copy.table.label });
    const you = await within(t).findByRole("button", { name: new RegExp(`^${copy.groups.you}`) });
    expect(you).toHaveTextContent(/· ≥ \$[\d,]+\.\d\d$/);
  });
});
