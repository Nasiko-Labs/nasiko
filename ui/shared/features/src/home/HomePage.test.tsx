// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/HomePage.test.tsx. Local edits: the
// account-failure case is not ported: the SPA shell resolves `/api/me` before the page mounts, so the page never sees a
// failed account read; Prettier formatting.
/**
 * The scoped home at `/` (plans/feat-agent-centric.md PR2a; design "Home"; eng "URL rule", "Fleet roll-up honesty",
 * "Members"). It keeps the Overview's account, first-run and Needs you behaviour, and adds the scope, the totals strip
 * and the agent table.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { delay, http, HttpResponse } from "msw";
import { afterEach, describe, expect, it } from "vitest";
import { copy as overviewCopy } from "../overview/copy";
import { configureMocks } from "@nasiko/mocks/handlers";
import { now, seed, setupPinnedSeed } from "@nasiko/testing/pinned-seed";
import { renderApp } from "@nasiko/app-shell/testing/render-app";
import { recordRequests, server } from "@nasiko/testing/pinned-seed";
import { copy } from "./copy";

setupPinnedSeed();
afterEach(() =>
  configureMocks({
    seed,
    now,
    loggedIn: true,
    variant: null,
    persona: null,
    chatVariants: [],
    onboarding: { persona: null, completed: true },
  }),
);

const as = (persona: string | null) =>
  configureMocks({ onboarding: { persona: persona as never, completed: true } });
const table = () => screen.findByRole("table", { name: copy.table.label });
/** The dollar figure in a cell ("$22.32 est." → 22.32; "—" → 0). */
const usd = (text: string | null | undefined) =>
  Number((text?.match(/[\d,]+\.\d+/)?.[0] ?? "0").replace(/,/g, ""));
const scopeItem = (name: string) =>
  within(screen.getByRole("radiogroup", { name: copy.scope.label })).getByRole("radio", { name });

describe("scope", () => {
  it("a developer starts on Yours: only their own agents and harnesses", async () => {
    as("developer");
    const { router } = renderApp("/");
    await table();
    expect(scopeItem(copy.scope.yours)).toHaveAttribute("aria-checked", "true");
    expect(router.state.location.search).not.toHaveProperty("scope");
    const owners = within(await table())
      .getAllByRole("row")
      .slice(1)
      .map((r) => within(r).getAllByRole("rowheader")[0]!.textContent);
    expect(owners.every((t) => t?.includes(copy.table.you))).toBe(true);
  });

  it("an SRE starts on Fleet, written to the URL once", async () => {
    as("sre");
    const { router } = renderApp("/");
    await table();
    await waitFor(() => expect(router.state.location.search).toMatchObject({ scope: "fleet" }));
  });

  it("a user’s choice is never overwritten when the persona answers later", async () => {
    let release: () => void = () => {};
    const gate = new Promise<void>((r) => (release = r));
    server.use(
      http.get("/api/me/onboarding", async () => {
        await gate;
        return HttpResponse.json({ is_first_time_user: false, persona: "sre" });
      }),
    );
    const { router } = renderApp("/");
    await screen.findByRole("radiogroup", { name: copy.scope.label });
    await userEvent.click(scopeItem(copy.scope.yours));
    release();
    await table();
    await new Promise((r) => setTimeout(r, 100));
    expect(router.state.location.search).toMatchObject({ scope: "yours" });
    expect(scopeItem(copy.scope.yours)).toHaveAttribute("aria-checked", "true");
  });

  it("the last choice is remembered for this user", async () => {
    as("developer");
    renderApp("/");
    await table();
    await userEvent.click(scopeItem(copy.scope.fleet));
    await table();
    renderApp("/");
    await waitFor(() =>
      expect(screen.getAllByRole("radio", { name: copy.scope.fleet }).at(-1)).toHaveAttribute(
        "aria-checked",
        "true",
      ),
    );
  });

  it("owning nothing says so and offers the fleet", async () => {
    const page = (await (await fetch("/api/agents?limit=100&offset=0")).json()) as Record<
      string,
      unknown
    >[];
    const others = page.slice(0, 3).map((a) => ({ ...a, owner_id: "someone-else" }));
    server.use(
      http.get("/api/agents", ({ request }) =>
        HttpResponse.json(
          Number(new URL(request.url).searchParams.get("offset") ?? 0) ? [] : others,
        ),
      ),
    );
    renderApp("/?scope=yours");
    expect(await screen.findByText(copy.empty.noOwned)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: copy.empty.showFleet }));
    // The header row, one owner group's header row, and its three agents.
    const rows = within(await table()).getAllByRole("row");
    expect(rows.filter((r) => !r.hasAttribute("data-group"))).toHaveLength(4);
    expect(rows.filter((r) => r.hasAttribute("data-group"))).toHaveLength(1);
  });
});

describe("rows and totals", () => {
  // Yours: the superuser's fleet has more than 100 rows, which the table virtualises (no layout in jsdom).
  it("sorts by needs, then spend, and a header click resorts", async () => {
    renderApp("/?scope=yours");
    const t = await table();
    await waitFor(() => expect(within(t).getAllByRole("row").length).toBeGreaterThan(3));
    const spendCol = (row: HTMLElement) =>
      (within(row).getAllByRole("cell").at(2)?.textContent ?? "").match(/[\d,]+\.\d+/)?.[0] ?? "0";
    const rows = () => within(t).getAllByRole("row").slice(1);
    // The crashed agent needs action and leads.
    await waitFor(() => expect(rows()[0]).toHaveTextContent(overviewCopy.rating.action));
    await userEvent.click(within(t).getByRole("button", { name: /Spend 30d/ }));
    const money = rows().map((r) => Number(spendCol(r).replace(/,/g, "")));
    const asc = [...money].sort((a, b) => a - b);
    expect([asc, [...asc].reverse()]).toContainEqual(money);
  });

  it("totals are the sum of the rows shown, and harness spend keeps its estimate basis", async () => {
    renderApp("/?scope=yours");
    const strip = await screen.findByRole("region", { name: copy.strip.label });
    const t = await table();
    await waitFor(() => expect(within(strip).queryByText("—")).toBeNull());
    const sum = within(t)
      .getAllByRole("row")
      .slice(1)
      .map((r) => usd(within(r).getAllByRole("cell").at(2)?.textContent))
      .reduce((a, b) => a + b, 0);
    const spendTile = within(strip).getByText(copy.strip.spend).closest('[data-slot="card"]')!;
    const shown = usd(within(spendTile as HTMLElement).getByText(/^(≥ )?\$/).textContent);
    expect(Math.abs(shown - sum)).toBeLessThan(
      0.02 * Math.max(1, within(t).getAllByRole("row").length),
    );
    expect(within(strip).getByText(copy.strip.estimates)).toBeInTheDocument();
    expect(within(t).getAllByText(copy.table.harness).length).toBeGreaterThan(0);
  });

  it("each row opens its agent", async () => {
    renderApp("/?scope=yours");
    const t = await table();
    const link = (await within(t).findAllByRole("link"))[0]!;
    expect(link.getAttribute("href")).toMatch(/^\/agents\/5eed/);
  });
});

describe("members", () => {
  it("never read the fleet-wide spend series, and Fleet says it is what is shared with them", async () => {
    configureMocks({ persona: "sam" });
    const rec = recordRequests();
    renderApp("/?scope=fleet");
    await table();
    expect(await screen.findByText(copy.scope.sharedWithYou)).toBeInTheDocument();
    await new Promise((r) => setTimeout(r, 200));
    rec.stop();
    expect(
      rec.urls.filter(
        (u) => u.pathname.endsWith("/finops/spend-timeseries") && !u.searchParams.get("agent_id"),
      ),
    ).toEqual([]);
  });
});

describe("kept from the Overview", () => {
  it("a failed agent list is one error with Retry", async () => {
    server.use(http.get("/api/agents", () => new HttpResponse("boom", { status: 500 })));
    renderApp("/");
    expect(await screen.findByText(copy.error, {}, { timeout: 15_000 })).toBeInTheDocument();
  });

  it("an empty fleet gets the first-run layout and reads no money", async () => {
    configureMocks({ variant: "onboarding-absent" });
    server.use(http.get("*/api/agents", () => HttpResponse.json([])));
    const rec = recordRequests();
    renderApp("/");
    expect(
      await screen.findByRole("heading", { level: 2, name: overviewCopy.firstRun.title }),
    ).toBeInTheDocument();
    rec.stop();
    expect(rec.urls.filter((u) => u.pathname.includes("/finops/"))).toEqual([]);
  });

  it("Needs you leads the page", async () => {
    renderApp("/");
    const needs = await screen.findByTestId("overview-needs");
    const t = await table();
    expect(needs.compareDocumentPosition(t) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});

describe("money states (review)", () => {
  const spendTile = async () =>
    within(await screen.findByRole("region", { name: copy.strip.label }))
      .getByText(copy.strip.spend)
      .closest('[data-slot="card"]') as HTMLElement;

  it('a window change shows "—" until the new window answers, never the old figures under the new label', async () => {
    renderApp("/?scope=yours");
    await table();
    await waitFor(async () => expect(await spendTile()).toHaveTextContent("$"));
    server.use(
      http.get("*/api/observability/finops/dashboard", async ({ request }) => {
        // 90d is an explicit window nothing else reads (7d and 30d are warm from the ratings' own reads).
        if (!new URL(request.url).searchParams.get("start_time")) return undefined;
        await delay("infinite");
        return HttpResponse.json({});
      }),
    );
    await userEvent.click(screen.getByRole("radio", { name: overviewCopy.range.item(90) }));
    await waitFor(async () => expect(await spendTile()).toHaveTextContent("—"));
    expect(screen.getByRole("columnheader", { name: /Spend 90d/ })).toBeInTheDocument();
  });

  it('a failed spend read says so, shows "—", and Retry brings the figures back', async () => {
    let fail = true;
    server.use(
      http.get("*/api/observability/finops/dashboard", ({ request }) =>
        fail && new URL(request.url).searchParams.get("range") === "30d"
          ? new HttpResponse("boom", { status: 500 })
          : undefined,
      ),
    );
    renderApp("/?scope=yours");
    const line = await screen.findByText(copy.moneyFailed, {}, { timeout: 15_000 });
    expect(await spendTile()).toHaveTextContent("—");
    fail = false;
    await userEvent.click(within(line).getByRole("button", { name: copy.retry }));
    await waitFor(async () => expect(await spendTile()).toHaveTextContent("$"));
  });

  it('unpriced calls make spend a floor, and harness rows carry "est."', async () => {
    renderApp("/?scope=yours");
    const t = await table();
    await waitFor(async () => expect(await spendTile()).toHaveTextContent("$"));
    const tile = await spendTile();
    expect(within(tile).getByText(copy.strip.unpriced)).toBeInTheDocument();
    expect(tile).toHaveTextContent("≥ $");
    const harnessRow = within(t)
      .getAllByRole("row")
      .find((r) => within(r).queryByText(copy.table.harness))!;
    const agentRow = within(t)
      .getAllByRole("row")
      .slice(1)
      .find((r) => !within(r).queryByText(copy.table.harness))!;
    const spendCell = (r: HTMLElement) => within(r).getAllByRole("cell")[2]!;
    expect(spendCell(harnessRow)).toHaveTextContent(copy.table.estimate);
    expect(spendCell(agentRow)).not.toHaveTextContent(copy.table.estimate);
  });

  it("a superuser’s ratings do read the fleet series (the members test’s control)", async () => {
    const rec = recordRequests();
    renderApp("/?scope=yours");
    await table();
    await waitFor(() =>
      expect(
        rec.urls.some(
          (u) => u.pathname.endsWith("/finops/spend-timeseries") && !u.searchParams.get("agent_id"),
        ),
      ).toBe(true),
    );
    rec.stop();
  });

  it("a header click sorts that column, most first", async () => {
    renderApp("/?scope=yours");
    const t = await table();
    await waitFor(async () => expect(await spendTile()).toHaveTextContent("$"));
    await userEvent.click(within(t).getByRole("button", { name: /Spend 30d/ }));
    const money = within(t)
      .getAllByRole("row")
      .slice(1)
      .map((r) => usd(within(r).getAllByRole("cell")[2]?.textContent));
    expect(money).toEqual([...money].sort((a, b) => b - a));
  });
});

describe("org levels without an edition layer (OSS)", () => {
  it("a unit link says it isn’t a unit the viewer can see, and offers the fleet", async () => {
    renderApp("/?scope=unit:5eed0014-0000-4000-8000-000000000001");
    expect(await screen.findByText(copy.empty.unknownUnit)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: copy.empty.showFleet }));
    expect(await table()).toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: copy.scope.unitLabel })).toBeNull();
  });
});

describe("a unit link on an empty fleet (review)", () => {
  it("says the unit isn’t visible rather than showing the first run", async () => {
    server.use(http.get("*/api/agents", () => HttpResponse.json([])));
    renderApp("/?scope=unit:5eed0014-0000-4000-8000-000000000001");
    expect(await screen.findByText(copy.empty.unknownUnit)).toBeInTheDocument();
    expect(
      screen.queryByRole("heading", { level: 2, name: overviewCopy.firstRun.title }),
    ).toBeNull();
    // Needs you reads no sessions for an empty fleet: it isn't drawn, rather than loading forever.
    await waitFor(() => expect(screen.queryByTestId("overview-needs")).toBeNull());
  });
});
