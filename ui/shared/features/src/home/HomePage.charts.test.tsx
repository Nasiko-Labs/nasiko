// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/HomePage.charts.test.tsx. Local edits: Prettier formatting.
/**
 * The home's chart band (plans/feat-agent-centric.md PR2d, decisions #59-63): spend over time for superusers only (the
 * series isn't ACL-scoped), by agent for everyone, TokenOps' Cost vs performance, saved tokens by agent, and the health
 * toggles that narrow the table through `?health=` while the strip and charts keep the whole scope.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http } from "msw";
import { afterEach, describe, expect, it } from "vitest";
import { configureMocks } from "@nasiko/mocks/handlers";
import { now, seed, setupPinnedSeed } from "@nasiko/testing/pinned-seed";
import { renderApp } from "@nasiko/app-shell/testing/render-app";
import { recordRequests, server } from "@nasiko/testing/pinned-seed";
import { copy } from "./copy";

setupPinnedSeed();
afterEach(() => {
  configureMocks({
    seed,
    now,
    loggedIn: true,
    variant: null,
    persona: null,
    superuser: null,
    onboarding: { persona: null, completed: true },
    budgetVariants: [],
  });
});

const T = { timeout: 5000 };
const band = () => screen.findByRole("region", { name: copy.charts.label }, T);
const panel = async (title: string) =>
  (await within(await band()).findByRole("heading", { name: title }, T)).closest("section")!;
const table = () => screen.findByRole("table", { name: copy.table.label }, T);
/** The table's agent rows: each names its agent with a link to the agent's hub (group and header rows don't). */
const dataRows = async () =>
  within(await table())
    .getAllByRole("row")
    .filter((r) =>
      within(r)
        .queryAllByRole("link")
        .some((a) => /^\/agents\/[^?]/.test(a.getAttribute("href") ?? "")),
    );

describe("spend", () => {
  // Over time shows where the Budget burn panel doesn't (plans/feat-budget-burn.md D13): on a server without budgets.
  it("a superuser sees spend over time, stacked from the top agents’ series plus the fleet’s on Fleet", async () => {
    configureMocks({ budgetVariants: ["budgets-absent"] });
    const rec = recordRequests();
    renderApp("/?scope=fleet");
    const spend = await panel(copy.charts.spend(30));
    expect(within(spend).getByRole("radio", { name: copy.charts.overTime })).toBeChecked();
    await waitFor(() => expect(spend).toHaveTextContent(copy.charts.other));
    rec.stop();
    const series = rec.urls.filter((u) => u.pathname.endsWith("/finops/spend-timeseries"));
    // One fleet read (no agent_id) for Other, and at most five agent reads for the stack.
    expect(series.filter((u) => !u.searchParams.get("agent_id")).length).toBeGreaterThan(0);
    expect(within(spend).getByText(copy.charts.trendSummary(5, 30, true))).toBeInTheDocument();

    await userEvent.click(within(spend).getByRole("radio", { name: copy.charts.byAgent }));
    expect(within(spend).getAllByRole("listitem").length).toBeGreaterThan(1);
    // Each bar opens its agent.
    expect(within(spend).getAllByRole("link")[0]).toHaveAttribute(
      "href",
      expect.stringMatching(/^\/agents\//),
    );
  });

  it("Over time on Yours draws no Other: the fleet total isn’t the rest of the viewer’s agents", async () => {
    configureMocks({ budgetVariants: ["budgets-absent"] });
    renderApp("/?scope=yours");
    const spend = await panel(copy.charts.spend(30));
    await waitFor(() => expect(within(spend).queryAllByRole("figure").length).toBe(1));
    expect(within(spend).queryByText(copy.charts.other)).toBeNull();
    expect(spend).not.toHaveTextContent(/rest of the fleet/);
  });

  it("a member gets spend by agent only and never reads a spend series", async () => {
    configureMocks({ superuser: false });
    const rec = recordRequests();
    renderApp("/?scope=fleet");
    const spend = await panel(copy.charts.spend(30));
    await waitFor(() => expect(within(spend).getAllByRole("listitem").length).toBeGreaterThan(1));
    expect(within(spend).queryByRole("radio")).toBeNull();
    await new Promise((r) => setTimeout(r, 200));
    rec.stop();
    expect(rec.urls.filter((u) => u.pathname.endsWith("/finops/spend-timeseries"))).toEqual([]);
  });
});

describe("the other panels", () => {
  it("draws Cost vs performance and saved tokens by agent from the rows", async () => {
    renderApp("/?scope=fleet");
    await panel("Cost vs performance");
    const saved = await panel(copy.charts.saved);
    await waitFor(() => expect(within(saved).getAllByRole("listitem").length).toBeGreaterThan(1));
  });
});

describe("health filter", () => {
  it("a rating narrows the table only, says so, and Clear brings every row back", async () => {
    // Yours: under the table's virtualisation threshold, so every row is in the DOM.
    renderApp("/?scope=yours");
    const health = await panel(copy.charts.health);
    await waitFor(async () => expect((await dataRows()).length).toBeGreaterThan(0));
    const all = (await dataRows()).length;
    const healthy = await within(health).findByRole("radio", { name: /^Show only Healthy/ }, T);
    await userEvent.click(healthy);
    expect(healthy).toBeChecked();
    await waitFor(async () => expect((await dataRows()).length).toBeLessThan(all));
    for (const r of await dataRows()) expect(r).toHaveTextContent("Healthy");
    const note = screen.getByText(/^Showing \d+ of \d+: Healthy/);
    expect(note).toHaveTextContent(`of ${all}`);
    // The band still covers the whole scope.
    expect(within(health).getByRole("radio", { name: /^Show only Not rated/ })).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: copy.charts.clear }));
    await waitFor(async () => expect((await dataRows()).length).toBe(all));
    expect(screen.queryByText(/^Showing \d+ of/)).toBeNull();
  });

  it("a ?health= link opens with the table filtered", async () => {
    renderApp("/?scope=yours&health=watch");
    await screen.findByText(/^Showing \d+ of \d+: Watch/, undefined, T);
    for (const r of await dataRows()) expect(r).toHaveTextContent("Watch");
  });

  it('a ?health= link waits for the ratings: never "none match" while every row still reads Not rated', async () => {
    // Ratings read the fleet's spend series; hold it until released.
    let release = () => {};
    const held = new Promise<void>((r) => (release = r));
    server.use(
      http.get("*/api/observability/finops/spend-timeseries", async ({ request }) => {
        if (!new URL(request.url).searchParams.get("agent_id")) await held;
        return undefined;
      }),
    );
    renderApp("/?scope=yours&health=watch");
    await screen.findByText(copy.charts.filterPending("Watch"), undefined, T);
    expect(screen.queryByText(/^No rows are/)).toBeNull();
    // The table keeps every row meanwhile.
    expect((await dataRows()).length).toBeGreaterThan(0);
    release();
    await screen.findByText(/^Showing \d+ of \d+: Watch/, undefined, T);
    for (const r of await dataRows()) expect(r).toHaveTextContent("Watch");
  });
});
