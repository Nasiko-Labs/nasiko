// Ported from nasiko-ui-lab@1b99bdda common/src/features/tokenops/TokenopsPage.test.tsx. Local edits: `renderApp` -> `renderTokenops`, `router.state.location` ->
// `location()`, `router.navigate({ to, search })` -> `router.navigate(search)`; `recordRequests`/`server` from
// `@/test/pinned-seed`; the three sign-in cases are not ported (the /login page and the guard are the shell's)
// and the two 401 cases assert the query client's `onUnauthorized` and the cleared cache (the shell turns that
// into the redirect); Prettier formatting.

/**
 * Page-level tests: the real router + page + app QueryClient against the seed-backed MSW
 * handlers (same handlers as the browser). Time is pinned so results don't depend on the
 * run date.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { delay, http, HttpResponse } from "msw";
import { describe, expect, it } from "vitest";
import { configureMocks } from "@nasiko/mocks/handlers";
import { generateHarnessSeed } from "@nasiko/mocks/seed-harness";
import {
  FIXED,
  now,
  recordRequests,
  seed,
  section,
  server,
  setupPinnedSeed,
} from "@nasiko/testing/pinned-seed";
import { isProductKey, meQueryOptions } from "@nasiko/data-access/product";
import { location, MOCK_SCOPE, renderTokenops } from "./test-utils";

const FINOPS = "/api/observability/finops";

setupPinnedSeed();

const liveAgent = seed.agents.find((a) => !a.deleted)!;
const deletedAgent = seed.agents.find((a) => a.deleted)!;

describe("TokenOps page", () => {
  it("renders F1, KPIs, and an attribution table with display names only", async () => {
    renderTokenops("/tokenops?open=all");
    expect(await screen.findByTestId("mtd-spend")).toHaveTextContent(/\$/);
    const table = await within(await section("Who is driving cost")).findByRole("table");
    expect(within(table).getAllByText(liveAgent.display_name).length).toBeGreaterThan(0);
    expect(within(table).queryByText(liveAgent.name)).toBeNull();
  });

  it("shows savings per category and per agent, with both reduction percentages", async () => {
    renderTokenops("/tokenops?open=optimise");
    const panel = await section("Token optimisation savings");

    // Both percentages, always together: the gap between them is real (savings are input-side,
    // input is the cheap side) and showing only one invites the reader to assume they match.
    // Each appears twice — once in the hero, once on the category row that produced it.
    expect(within(panel).getAllByText("19.7%")).toHaveLength(2);
    expect(within(panel).getAllByText("11.0%")).toHaveLength(2);

    // The category split — "what did Caveman save vs Ponytail" — answered directly.
    expect(within(panel).getByText("Smaller prompts (Caveman)")).toBeInTheDocument();
    expect(within(panel).getByText("Less code written (Ponytail)")).toBeInTheDocument();

    // A zero category stays visible and says why, so it reads as "nobody turned it on" rather
    // than "this feature does nothing".
    expect(within(panel).getByText(/No coding agent has this turned on/)).toBeInTheDocument();

    expect(within(panel).getByRole("link", { name: /Turn on for an agent/ })).toHaveAttribute(
      "href",
      "/agents",
    );
  });

  it("lists the sessions the optimiser helped most", async () => {
    // Savings recur per turn, so a long session compounds them in a way the per-call view
    // understates. This table is where that shows, and it links back to the conversation.
    renderTokenops("/tokenops?open=optimise");
    const panel = await section("Token optimisation savings");
    const link = within(panel).getByRole("link", { name: /ctx-9f3ad21b/ });
    expect(link).toHaveAttribute("href", "/sessions/ctx-9f3ad21b0c74");
    expect(within(panel).getByText("22.4%")).toBeInTheDocument();
  });

  it("names the layers underneath a category that has more than one", async () => {
    // "Caveman saved this much" is the product question; which injection point carried it is the
    // engineering one, and both belong on the same row.
    renderTokenops("/tokenops?open=optimise");
    const panel = await section("Token optimisation savings");
    // Named for what got shorter, not for the injection point the ledger stores. Scoped to the
    // row, because the panel's own intro also talks about tool output in plain words.
    const row = within(panel).getByRole("rowheader", { name: /Smaller prompts/ });
    expect(row).toHaveTextContent(/tool output/);
    expect(row).toHaveTextContent(/chat history/);
    expect(row).not.toHaveTextContent(/compress_/);
  });

  it("marks a figure that is estimated rather than measured", async () => {
    // The minimal-code row has no observable counterfactual, so its number is a factor applied to
    // counted traffic. That has to be visible rather than blended into the measured total.
    renderTokenops("/tokenops?open=optimise");
    const panel = await section("Token optimisation savings");
    expect(within(panel).getAllByLabelText("Estimated").length).toBeGreaterThan(0);
    expect(within(panel).getByText("Measured + estimated")).toBeInTheDocument();
  });

  it("shows one page loader under the sticky bar on a cold load; a window change keeps the panels", async () => {
    server.use(
      http.get(`*${FINOPS}/dashboard`, async () => {
        await delay(300);
        return undefined;
      }),
    );
    const { router } = renderTokenops("/tokenops?open=all");
    // The header is static, so it stays above the loader.
    await screen.findByRole("heading", { level: 1, name: "TokenOps" });
    expect(screen.getByTestId("page-loader")).toBeInTheDocument();
    expect(screen.queryByText("Spend over time")).toBeNull();
    await screen.findByTestId("mtd-spend");
    expect(screen.queryByTestId("page-loader")).toBeNull();
    router.navigate({ open: "all", preset: "7d" });
    expect(screen.queryByTestId("page-loader")).toBeNull();
    expect(screen.getByTestId("mtd-spend")).toBeInTheDocument();
  });

  it("sends range only for rolling presets, explicit UTC bounds for This month", async () => {
    const rec = recordRequests();
    try {
      const { router } = renderTokenops("/tokenops?open=all&preset=7d");
      await screen.findByTestId("mtd-spend");
      await waitFor(() => {
        expect(
          rec.urls.some(
            (u) => u.pathname.endsWith("/dashboard") && u.searchParams.get("range") === "7d",
          ),
        ).toBe(true);
      });
      router.navigate({ open: "all", preset: "mtd", view: "agent", sort: "cost" });
      await waitFor(() => {
        const mtd = rec.urls.find(
          (u) =>
            u.pathname.endsWith("/dashboard") &&
            u.searchParams.get("start_time") === "2026-03-01T00:00:00.000Z",
        );
        expect(mtd).toBeTruthy();
        expect(mtd?.searchParams.get("range")).toBeNull();
      });
    } finally {
      rec.stop();
    }
  });

  it("clicking a calendar day opens the day panel; Back closes it", async () => {
    const user = userEvent.setup();
    const { router } = renderTokenops("/tokenops?open=all");
    const grid = await screen.findByRole("grid");
    const firstDay = within(grid)
      .getAllByRole("button")
      .find((b) => !b.hasAttribute("disabled"))!;
    await user.click(firstDay);
    expect(await screen.findByRole("heading", { name: /hour by hour/ })).toBeInTheDocument();
    expect(location().search).toHaveProperty("day");
    router.back();
    await waitFor(() =>
      expect(screen.queryByRole("heading", { name: /hour by hour/ })).not.toBeInTheDocument(),
    );
  });

  it("spike day: names the spike agent, Table view filters by agent UUID, View traces opens the drawer", async () => {
    const user = userEvent.setup();
    const spike = seed.agents.find((a) => a.name === seed.spikeAgentName)!;
    renderTokenops(`/tokenops?open=all&day=${seed.spikeDate}`);
    const list = await screen.findByRole("list", { name: "Top agents this day" });
    // Day slices carry display names, like the real server.
    expect(within(list).getByText(spike.display_name)).toBeInTheDocument();
    await user.click(within(await section(/hour by hour/)).getByRole("button", { name: "Table" }));
    const hours = within(await section(/hour by hour/)).getByRole("table");
    expect(within(hours).getByRole("columnheader", { name: "Hour (UTC)" })).toBeInTheDocument();
    await user.click(within(hours).getByRole("button", { name: spike.display_name }));
    // The display name is mapped to the agent's UUID: the server rejects display names as agent_id.
    await waitFor(() => {
      expect(location().search).toMatchObject({ agent: spike.id });
    });
    await user.click(
      within(await section(/hour by hour/)).getByRole("button", { name: "View traces" }),
    );
    const drawer = await screen.findByRole("dialog");
    expect(within(drawer).getByRole("heading", { name: "Most expensive traces" })).toHaveFocus();
    expect(await within(drawer).findAllByText(/Open trace/)).not.toHaveLength(0);
  });

  it("F4: a 404 explains the missing endpoint", async () => {
    server.use(
      http.get(`${FINOPS}/top-traces`, () => new HttpResponse("not found", { status: 404 })),
    );
    renderTokenops("/tokenops?open=all&traces=true");
    expect(await screen.findByText(/needs a newer nasiko-server/)).toBeInTheDocument();
  });

  it("F4: a 500 offers Retry, not the newer-server message", async () => {
    server.use(
      http.get(`${FINOPS}/top-traces`, () => new HttpResponse("internal error", { status: 500 })),
    );
    renderTokenops("/tokenops?open=all&traces=true");
    const drawer = await screen.findByRole("dialog");
    expect(await within(drawer).findByRole("button", { name: /Retry/ })).toBeInTheDocument();
    expect(within(drawer).queryByText(/newer nasiko-server/)).toBeNull();
  });

  it("F4: an empty result shows the empty state", async () => {
    server.use(
      http.get(`${FINOPS}/top-traces`, () =>
        HttpResponse.json({ data: { rows: [], has_more: false }, status_code: 200, message: "ok" }),
      ),
    );
    renderTokenops("/tokenops?open=all&traces=true");
    expect(await screen.findByText("No traces in this scope")).toBeInTheDocument();
  });

  it("a failed dashboard shows a panel error with Retry while the month hero still renders", async () => {
    server.use(
      http.get(`${FINOPS}/dashboard`, () => new HttpResponse("internal error", { status: 500 })),
    );
    renderTokenops("/tokenops?open=all");
    expect(await screen.findByTestId("mtd-spend")).toBeInTheDocument();
    const alerts = await screen.findAllByRole("alert");
    expect(alerts.some((a) => /Couldn't load KPIs/.test(a.textContent ?? ""))).toBe(true);
    expect(screen.getAllByRole("button", { name: /Retry/ }).length).toBeGreaterThan(0);
  });

  it("previous-window failure shows Δ as unavailable, not a made-up number", async () => {
    server.use(
      http.get(`${FINOPS}/dashboard`, ({ request }) => {
        const u = new URL(request.url);
        // The previous-window query is the one with explicit bounds while the page is on 30d.
        if (!u.searchParams.get("range") && u.searchParams.get("start_time"))
          return new HttpResponse("internal error", { status: 500 });
        return undefined;
      }),
    );
    renderTokenops("/tokenops?open=all");
    expect((await screen.findAllByLabelText("comparison unavailable")).length).toBeGreaterThan(0);
  });

  it("the latest window wins when an older response arrives later", async () => {
    let slowStarted!: () => void;
    const slowInFlight = new Promise<void>((resolve) => {
      slowStarted = resolve;
    });
    server.use(
      http.get(`${FINOPS}/dashboard`, async ({ request }) => {
        if (new URL(request.url).searchParams.get("range") === "7d") {
          slowStarted();
          await delay(300);
        }
        return undefined;
      }),
    );
    const { router, queryClient } = renderTokenops("/tokenops?open=all&preset=7d");
    // Navigate only once the 7d request is really in flight, so its late answer races the 24h one.
    await slowInFlight;
    router.navigate({ open: "all", preset: "24h", view: "agent", sort: "cost" });
    expect(await screen.findByLabelText("Key metrics, Last 24 hours")).toBeInTheDocument();
    // Settled: the 7d request has either landed in its own cache entry or been cancelled.
    await waitFor(
      () => {
        expect(queryClient.isFetching()).toBe(0);
      },
      { timeout: 8000 },
    );
    expect(screen.getByLabelText("Key metrics, Last 24 hours")).toBeInTheDocument();
    expect(screen.queryByLabelText("Key metrics, Last 7 days")).toBeNull();
  });

  it("unknown agent filter (400) explains and offers to clear it", async () => {
    const user = userEvent.setup();
    renderTokenops("/tokenops?open=all&agent=does-not-exist");
    expect((await screen.findAllByText(/unknown agent 'does-not-exist'/)).length).toBeGreaterThan(
      0,
    );
    await user.click(screen.getByRole("button", { name: "Clear agent filter" }));
    await waitFor(() => {
      expect(location().search).not.toHaveProperty("agent");
    });
  });

  it("a deleted agent is unknown to the mock too (400), like the real resolve_agent", async () => {
    renderTokenops(`/tokenops?open=all&agent=${deletedAgent.name}`);
    expect(await screen.findByRole("button", { name: "Clear agent filter" })).toBeInTheDocument();
  });

  it('an inaccessible agent (404 "agent not found") gets the same notice', async () => {
    server.use(
      http.get(`${FINOPS}/dashboard`, ({ request }) =>
        new URL(request.url).searchParams.get("agent_id")
          ? new HttpResponse("agent not found", { status: 404 })
          : undefined,
      ),
    );
    renderTokenops(`/tokenops?open=all&agent=${liveAgent.id}`);
    expect(await screen.findByRole("button", { name: "Clear agent filter" })).toBeInTheDocument();
  });

  it("/api/me 502 shows the server-down page, not endless skeletons", async () => {
    server.use(http.get("/api/me", () => new HttpResponse("bad gateway", { status: 502 })));
    renderTokenops("/tokenops?open=all");
    expect(await screen.findByText(/Can't reach nasiko-server/)).toBeInTheDocument();
  });

  it("a dashboard network failure shows the server-down page", async () => {
    server.use(http.get(`${FINOPS}/dashboard`, () => HttpResponse.error()));
    renderTokenops("/tokenops?open=all");
    expect(await screen.findByText(/Can't reach nasiko-server/)).toBeInTheDocument();
  });

  it("impossible or future custom dates fall back to 30d instead of crashing", async () => {
    renderTokenops("/tokenops?open=all&preset=custom&from=2026-02-30&to=2026-03-02");
    expect(await screen.findByLabelText("Key metrics, Last 30 days")).toBeInTheDocument();
  });

  it("future custom dates fall back to 30d", async () => {
    renderTokenops("/tokenops?open=all&preset=custom&from=2026-04-01&to=2026-04-05");
    expect(await screen.findByLabelText("Key metrics, Last 30 days")).toBeInTheDocument();
  });

  // Not ported: "no session: redirects to login with a relative redirect and no "expired" copy", "login round-trip
  // lands back on the original URL with its search intact" and "login shows 401 and 429 errors, and an off-site
  // redirect is ignored" render the /login page and the route guard, which are the shell's (app/(app)/layout.tsx).

  it("a 401 mid-session clears the cache and asks for the expired redirect", async () => {
    const { router, queryClient, onUnauthorized } = renderTokenops("/tokenops?open=all&preset=7d");
    await screen.findByTestId("mtd-spend");
    const tokenopsEntries = () =>
      queryClient
        .getQueryCache()
        .getAll()
        .filter((q) => isProductKey(q.queryKey) && q.queryKey[4] === "tokenops");
    // The cache is cleared before the shell is asked to redirect (the page is still mounted here, so its
    // observers would re-register afterwards): read it at that moment.
    const entriesAtRedirect: number[] = [];
    onUnauthorized.mockImplementation(() => {
      entriesAtRedirect.push(tokenopsEntries().length);
    });
    configureMocks({ loggedIn: false });
    router.navigate({ open: "all", preset: "24h", view: "agent", sort: "cost" });
    await waitFor(() => {
      expect(onUnauthorized).toHaveBeenCalled();
    });
    // The page stays mounted here (the shell would have navigated away), so its observers re-register and a
    // second burst follows; what matters is that the first redirect was asked for over an emptied cache.
    expect(entriesAtRedirect[0]).toBe(0);
  });

  it("a 401 on /api/me after sign-in is an expiry too, not a Retry panel", async () => {
    const { queryClient, onUnauthorized } = renderTokenops("/tokenops?open=all");
    await screen.findByTestId("mtd-spend");
    configureMocks({ loggedIn: false });
    await queryClient.refetchQueries({ queryKey: meQueryOptions(MOCK_SCOPE).queryKey });
    await waitFor(() => {
      expect(onUnauthorized).toHaveBeenCalledTimes(1);
    });
    expect(screen.queryByText("Couldn't load your session")).toBeNull();
  });

  it("shows first-run guidance when there is no spend in the last two months", async () => {
    // The admin's harness turns count as spend too (aggregate.ts withHarnessTurns), so empty both seeds.
    configureMocks({
      seed: { ...seed, agents: [], traces: [], sessions: [] },
      harnessSeed: { ...generateHarnessSeed({ anchor: FIXED }), sessions: [] },
    });
    try {
      renderTokenops("/tokenops?open=all");
      expect(await screen.findByText("No AI spend recorded yet")).toBeInTheDocument();
      expect(screen.getByRole("heading", { level: 1, name: "TokenOps" })).toBeInTheDocument();
    } finally {
      configureMocks({ harnessSeed: generateHarnessSeed({ anchor: FIXED }) });
    }
  });

  it('an empty window offers "Show last 30 days"', async () => {
    const user = userEvent.setup();
    configureMocks({
      seed: { ...seed, traces: seed.traces.filter((t) => t.ts < now() - 2 * 86_400_000) },
    });
    renderTokenops("/tokenops?open=all&preset=24h");
    const buttons = await screen.findAllByRole("button", { name: "Show last 30 days" });
    await user.click(buttons[0]!);
    await waitFor(() => {
      // 30d is the page's default, and defaults are left out of the URL (lib/url-state.ts).
      expect(location().search).not.toHaveProperty("preset");
    });
    expect(await screen.findByLabelText("Key metrics, Last 30 days")).toBeInTheDocument();
  });

  it("Workflows refetches with view=workflow and labels latency as an average", async () => {
    const rec = recordRequests();
    try {
      const { router } = renderTokenops("/tokenops?open=all");
      await section("Who is driving cost");
      router.navigate({ open: "all", preset: "30d", view: "workflow", sort: "cost" });
      await waitFor(() => {
        expect(rec.urls.some((u) => u.searchParams.get("view") === "workflow")).toBe(true);
      });
      expect(await screen.findAllByText(seed.workflows[0]!.workflow_name)).not.toHaveLength(0);
      expect(
        within(await section("Who is driving cost")).getByRole("columnheader", {
          name: /Avg latency/,
        }),
      ).toBeInTheDocument();
    } finally {
      rec.stop();
    }
  });
});
