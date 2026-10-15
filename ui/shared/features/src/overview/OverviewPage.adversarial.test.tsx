// Ported from nasiko-ui-lab@1b99bdda common/src/features/overview/OverviewPage.adversarial.test.tsx. Local edits: the
// TanStack memory router around the card is the navigation-port test harness (`renderWithRouter` from @nasiko/testing/router);
// `closest<HTMLElement>` in place of an `as` cast (lint); Prettier formatting.
/**
 * Fixes from the /ship adversarial review of the Overview (2026-09-29): the paths that let the page claim an
 * all-clear it hadn't checked, and the smaller staleness and labelling gaps.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderWithRouter } from "@nasiko/testing/router";
import { bypass, http, HttpResponse } from "msw";
import { afterEach, describe, expect, it, vi } from "vitest";
import { budgetMockState, configureMocks } from "@nasiko/mocks/handlers";
import { agentsList } from "@nasiko/mocks/observability";
import { now, seed, setupPinnedSeed } from "@nasiko/testing/pinned-seed";
import { renderApp } from "@nasiko/app-shell/testing/render-app";
import { server } from "@nasiko/testing/pinned-seed";
import type { NeedsYou as NeedsYouData } from "./api";
import { NeedsYou } from "./components/NeedsYou";
import { copy } from "./copy";
import { mergeNeeds } from "./needs";

setupPinnedSeed();
afterEach(() =>
  configureMocks({ seed, now, loggedIn: true, superuser: null, variant: null, routerVariants: [] }),
);

const card = (id: string) => screen.findByTestId(id);
const healthCounts = async () => {
  const health = await card("overview-health");
  await waitFor(
    () =>
      expect(
        within(health).getAllByRole("link", { name: /^\d+ (healthy|watch|needs? action|unknown)/ }),
      ).toHaveLength(4),
    { timeout: 5000 },
  );
  return Object.fromEntries(
    within(health)
      .getAllByRole("link", { name: /^\d+ (healthy|watch|needs? action|unknown)/ })
      .map((l) => {
        const [, n, what] = l.textContent.match(/^(\d+) (.*)$/)!;
        return [what!, Number(n)];
      }),
  );
};

describe("a failed cost input", () => {
  it('spend series down: Needs you keeps its rows but never says "Nothing needs you"', async () => {
    server.use(
      http.get(
        "*/api/observability/finops/spend-timeseries",
        () => new HttpResponse("boom", { status: 500 }),
      ),
    );
    renderApp("/");
    const needs = await card("overview-needs");
    expect(
      await within(needs).findByText(
        copy.couldntCheck(copy.needs.source.agents),
        {},
        { timeout: 5000 },
      ),
    ).toBeInTheDocument();
    expect(within(needs).queryByTestId("needs-empty")).toBeNull();
    expect(screen.getByTestId("overview-headline")).not.toHaveTextContent(copy.needs.nothing);
    // Known Needs-action agents (crashed, failed) still show.
    expect(needs.querySelectorAll('[data-kind="agent"]').length).toBeGreaterThan(0);
  });

  it("Retry on the agents line refetches the failed cost input, not just the agent list", async () => {
    let fail = true;
    server.use(
      http.get("*/api/observability/finops/spend-timeseries", () =>
        fail ? new HttpResponse("boom", { status: 500 }) : undefined,
      ),
    );
    renderApp("/");
    const needs = await card("overview-needs");
    const line = await within(needs).findByText(
      copy.couldntCheck(copy.needs.source.agents),
      {},
      { timeout: 5000 },
    );
    fail = false;
    await userEvent.click(
      within(line.closest<HTMLElement>("div.flex")!).getByRole("button", { name: copy.retry }),
    );
    await waitFor(
      () =>
        expect(within(needs).queryByText(copy.couldntCheck(copy.needs.source.agents))).toBeNull(),
      { timeout: 5000 },
    );
  });

  it("no comparison data: no agent is called Healthy", async () => {
    // The comparison window used to be a second dashboard request that could
    // fail on its own. It now rides on the current one (`compare=previous`), so
    // the live risk is a server that doesn't support the parameter and answers
    // without `previous` — the ratings must go Unknown, never Healthy.
    let served = 0;
    server.use(
      http.get("*/api/observability/finops/dashboard", async ({ request }) => {
        if (new URL(request.url).searchParams.get("compare") !== "previous") return undefined;
        served++;
        const res = await fetch(bypass(request));
        const body = (await res.json()) as { data: { agents: Record<string, unknown>[] } };
        body.data.agents = body.data.agents.map((a) => {
          const rest = { ...a };
          delete rest.previous;
          return rest;
        });
        return HttpResponse.json(body);
      }),
    );
    renderApp("/");
    const counts = await healthCounts();
    expect(served).toBeGreaterThan(0);
    expect(counts.healthy).toBe(0);
  });
});

describe("a stopped budget on an agent that is not rated", () => {
  // Budgets hidden (no server support for /api/budgets yet, R-L10): un-skip with the commented budget code.
  it.skip("still shows the budget in Needs you", async () => {
    const stopped = budgetMockState().budgets.find((b) => b.agent_id && b.action === "stop")!;
    server.use(
      http.get("*/api/agents", ({ request }) => {
        const u = new URL(request.url);
        const rows = agentsList(seed, {
          limit: u.searchParams.get("limit"),
          offset: u.searchParams.get("offset"),
        });
        return HttpResponse.json(
          rows.map((a) => (a.id === stopped.agent_id ? { ...a, status: "stopped" } : a)),
        );
      }),
    );
    renderApp("/");
    const needs = await card("overview-needs");
    await waitFor(
      () =>
        expect(
          [...needs.querySelectorAll('[data-kind="budget"]')].some((r) =>
            /over its limit/.test(r.textContent ?? ""),
          ),
        ).toBe(true),
      { timeout: 5000 },
    );
  });
});

describe("the Agents health filter with a failed agent list", () => {
  it('shows the error with Retry, not "no results"', async () => {
    server.use(
      http.get("*/api/agents", ({ request }) => {
        // The catalog pages (limit=50, the catalog query) succeed; the directory's full list fails.
        return new URL(request.url).searchParams.get("limit") === "100"
          ? new HttpResponse("boom", { status: 500 })
          : undefined;
      }),
    );
    renderApp("/agents?health=watch");
    expect(
      await screen.findByRole("button", { name: /retry/i }, { timeout: 5000 }),
    ).toBeInTheDocument();
    expect(screen.queryByText(/No agents match/i)).toBeNull();
  });
});

describe("Needs you on OSS (budgets absent)", () => {
  it("shows one card error when every available source failed", async () => {
    const data: NeedsYouData = {
      needs: mergeNeeds({
        requests: { state: "failed" },
        agents: { state: "failed" },
        budgets: { state: "absent" },
        sessions: { state: "failed" },
      }),
      lastChecked: 0,
      retry: { requests: vi.fn(), agents: vi.fn(), budgets: vi.fn(), sessions: vi.fn() },
      sessions: {} as NeedsYouData["sessions"],
    };
    renderWithRouter(<NeedsYou data={data} now={0} userId="u1" />, { url: "/" });
    expect(await screen.findByText(copy.couldntLoad(copy.needs.what))).toBeInTheDocument();
  });
});
