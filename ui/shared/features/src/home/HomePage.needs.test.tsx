// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/HomePage.needs.test.tsx. Local edits:
// the session-sample and previous-week cases follow development 480650a69 (failed sessions filtered in SQL, the
// comparison on `compare=previous`), as the Overview's own tests did; Prettier formatting.
/**
 * Needs you, first run and the ratings on the home (plans/feat-agent-centric.md PR2a): the Overview's own page tests for
 * the parts the home kept, moved here when the Overview page was retired (/review of PR2a: testing), plus the home's
 * rule that a failed or loading source never reads as 0 in its counts.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { delay, http, HttpResponse, bypass } from "msw";
import { afterEach, describe, expect, it } from "vitest";
import { clearChatRegistry } from "../chat/registry";
import { copy as deployCopy } from "../deploy/copy";
import { copy as overviewCopy } from "../overview/copy";
import { apiFetch } from "@nasiko/data-access/product";
import { configureChatMock } from "@nasiko/mocks/chatStore";
import { configureMocks } from "@nasiko/mocks/handlers";
import { now, seed, setupPinnedSeed } from "@nasiko/testing/pinned-seed";
import { renderApp } from "@nasiko/app-shell/testing/render-app";
import { recordRequests, server } from "@nasiko/testing/pinned-seed";
import { copy } from "./copy";

setupPinnedSeed();
afterEach(() => {
  configureMocks({ seed, now, loggedIn: true, superuser: null, variant: null });
  configureChatMock({ waiting: false });
  clearChatRegistry();
});

const c = overviewCopy;
const needs = () => screen.getByTestId("overview-needs");
const table = () => screen.findByRole("table", { name: copy.table.label });
const strip = () => screen.findByRole("region", { name: copy.strip.label });
/** Needs you has every source's answer (no labelled loading left). */
const settled = async () => {
  await table();
  await waitFor(() => expect(within(needs()).queryByLabelText(c.loading)).toBeNull(), {
    timeout: 5000,
  });
};
const needsTile = async () =>
  within(await strip())
    .getByText(copy.strip.needs)
    .closest('[data-slot="card"]') as HTMLElement;

describe("Needs you", () => {
  it("a superuser sees only requests provably theirs, with a link to each chat (eng R5)", async () => {
    configureChatMock({ waiting: true });
    renderApp("/?scope=yours");
    await settled();
    const rows = within(needs())
      .getAllByRole("listitem")
      .filter((li) => li.querySelector('[data-kind="request"]'));
    expect(rows.length).toBeGreaterThan(0);
    for (const r of rows)
      expect(
        within(r)
          .getByRole("link", { name: /Review the request in/ })
          .getAttribute("href"),
      ).toMatch(/^\/chat\//);
    expect(needs().querySelector('[data-kind="outside"]')).toBeNull();
  });

  it("a request row links its chat’s paused flow, without a second row (plans/feat-flows.md O2)", async () => {
    configureChatMock({ waiting: true });
    server.use(
      http.get("/api/flows", async ({ request }) => {
        if (new URL(request.url).searchParams.get("status") !== "paused") return undefined;
        const pending = await apiFetch<{
          data?: { execution: { chat_session_id: string | null } }[];
        }>("/api/hitl/pending");
        const chats = [
          ...new Set((pending.data ?? []).flatMap((r) => r.execution.chat_session_id ?? [])),
        ];
        return HttpResponse.json({
          data: chats.map((chat, i) => ({
            flow_id: `5eedf00000000000000000000000${String(i).padStart(4, "0")}`,
            status: "paused",
            root_agent_name: "orchestrator",
            metadata: { context_id: chat },
            created_at: "2026-03-20T14:00:00Z",
          })),
          total: chats.length,
        });
      }),
    );
    renderApp("/?scope=yours");
    await settled();
    const rows = within(needs())
      .getAllByRole("listitem")
      .filter((li) => li.querySelector('[data-kind="request"]'));
    expect(rows.length).toBeGreaterThan(0);
    await waitFor(() =>
      expect(within(rows[0]!).getByRole("link", { name: "Open flow" })).toHaveAttribute(
        "href",
        expect.stringMatching(/^\/flows\/5eedf/),
      ),
    );
    expect(needs().querySelectorAll('[data-kind="flow"]')).toHaveLength(0);
  });

  it("a failed paused-flows read only means no flow links: the inbox still shows (O2a)", async () => {
    configureChatMock({ waiting: true });
    server.use(
      http.get("/api/flows", ({ request }) =>
        new URL(request.url).searchParams.get("status") === "paused"
          ? new HttpResponse("internal error", { status: 500 })
          : undefined,
      ),
    );
    renderApp("/?scope=yours");
    await settled();
    const rows = within(needs())
      .getAllByRole("listitem")
      .filter((li) => li.querySelector('[data-kind="request"]'));
    expect(rows.length).toBeGreaterThan(0);
    expect(within(needs()).queryByRole("link", { name: "Open flow" })).toBeNull();
  });

  it('a normal user gets one "outside Chat" row for requests no chat claims (eng R5)', async () => {
    configureMocks({ superuser: false });
    configureChatMock({ waiting: true });
    renderApp("/?scope=yours");
    await settled();
    expect(needs().querySelector('[data-kind="outside"]')).toHaveTextContent(c.needs.outside(2));
  });

  it("a failed source adds a line, never an all-clear, and the home’s counts stay unknown (review)", async () => {
    server.use(http.get("/api/hitl/pending", () => new HttpResponse("boom", { status: 500 })));
    renderApp("/?scope=yours");
    await table();
    expect(
      await within(needs()).findByText(c.couldntCheck(c.needs.source.requests)),
    ).toBeInTheDocument();
    expect(within(needs()).queryByTestId("needs-empty")).toBeNull();
    // Every other source has answered, so the '—' below is the failed source's, not a load still in flight.
    await settled();
    await waitFor(() => expect(screen.queryByText(copy.table.checking)).toBeNull());
    expect(await needsTile()).toHaveTextContent("—");
    const t = await table();
    for (const row of within(t).getAllByRole("row").slice(1))
      expect(within(row).getAllByRole("cell")[1]).toHaveTextContent("—");
  });

  it("with every source answering, the home’s counts are numbers (the failed-source test’s control)", async () => {
    renderApp("/?scope=yours");
    await settled();
    await waitFor(async () => expect(await needsTile()).toHaveTextContent(/\d/), { timeout: 5000 });
    expect(await needsTile()).not.toHaveTextContent("—");
  });

  it("always says when it last checked (design 8A)", async () => {
    renderApp("/?scope=yours");
    await settled();
    expect(within(needs()).getByText(/^Checked /)).toBeInTheDocument();
  });

  // The checked sample this test bounded (eng R7: at most 25 status checks) became one filtered read: Needs you's
  // failed sessions come from `session/list?status=failed` (development 480650a69), so there is no fan-out at all.
  it("reads failed sessions from the list, with no per-session fan-out (eng R7)", async () => {
    const rec = recordRequests();
    renderApp("/?scope=yours");
    await settled();
    rec.stop();
    const details = new Set(
      rec.urls.map((u) => u.pathname).filter((p) => /\/session\/(?!list)[^/]+$/.test(p)),
    );
    expect(details.size).toBe(0);
    expect(
      rec.urls.some(
        (u) => u.pathname.endsWith("/session/list") && u.searchParams.get("status") === "failed",
      ),
    ).toBe(true);
  });
});

describe("ratings", () => {
  it("Needs you and the Health column wait for the rating dashboards, so nothing is called clear early", async () => {
    server.use(
      http.get("*/api/observability/finops/dashboard", async () => {
        await delay("infinite");
        return HttpResponse.json({});
      }),
    );
    renderApp("/?scope=yours");
    const t = await table();
    const loading = () =>
      within(needs()).queryByLabelText(c.loading) ?? within(needs()).queryByText(c.loading);
    await waitFor(() => expect(loading()).toBeInTheDocument());
    expect(within(needs()).queryByTestId("needs-empty")).toBeNull();
    expect(within(t).getAllByText(copy.table.checking).length).toBeGreaterThan(0);
    expect(within(t).queryByText(c.rating.healthy)).toBeNull();
  });

  // The previous week rides on the current dashboard (`compare=previous`, development 480650a69) instead of a read that
  // could fail on its own: a server that answers without `previous` is the missing comparison now.
  it("no comparison data: no agent is called Healthy on missing comparisons", async () => {
    let calls = 0;
    server.use(
      http.get("*/api/observability/finops/dashboard", async ({ request }) => {
        if (new URL(request.url).searchParams.get("compare") !== "previous") return undefined;
        calls++;
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
    renderApp("/?scope=yours");
    const t = await table();
    await waitFor(
      () => expect(within(t).getAllByText(c.rating.unknown).length).toBeGreaterThan(0),
      {
        timeout: 8000,
      },
    );
    expect(calls).toBeGreaterThan(0);
    expect(within(t).queryByText(c.rating.healthy)).toBeNull();
  });

  it('spend series down: Needs you keeps its rows but never says "Nothing needs you"', async () => {
    server.use(
      http.get(
        "*/api/observability/finops/spend-timeseries",
        () => new HttpResponse("boom", { status: 500 }),
      ),
    );
    renderApp("/?scope=yours");
    await table();
    expect(
      await within(needs()).findByText(
        c.couldntCheck(c.needs.source.agents),
        {},
        { timeout: 5000 },
      ),
    ).toBeInTheDocument();
    expect(within(needs()).queryByTestId("needs-empty")).toBeNull();
    expect(needs().querySelectorAll('[data-kind="agent"]').length).toBeGreaterThan(0);
  });

  it("Retry on the agents line refetches the failed cost input, not just the agent list", async () => {
    let fail = true;
    server.use(
      http.get("*/api/observability/finops/spend-timeseries", () =>
        fail ? new HttpResponse("boom", { status: 500 }) : undefined,
      ),
    );
    renderApp("/?scope=yours");
    await table();
    const line = await within(needs()).findByText(
      c.couldntCheck(c.needs.source.agents),
      {},
      { timeout: 5000 },
    );
    fail = false;
    await userEvent.click(
      within(line.closest<HTMLElement>("div.flex")!).getByRole("button", { name: c.retry }),
    );
    await waitFor(
      () => expect(within(needs()).queryByText(c.couldntCheck(c.needs.source.agents))).toBeNull(),
      { timeout: 5000 },
    );
  });

  it("a rating links to the Agents catalog filtered by it (the Overview’s Fleet health counts did)", async () => {
    renderApp("/?scope=yours");
    const t = await table();
    const badge = await within(t).findAllByRole("link", { name: c.rating.action });
    expect(badge[0]).toHaveAttribute("href", "/agents?health=action");
  });
});

describe("a failed agent list", () => {
  it("costs the table only: Needs you still loads and says agent health could not be checked", async () => {
    server.use(http.get("*/api/agents", () => new HttpResponse("boom", { status: 500 })));
    renderApp("/?scope=yours");
    expect(await screen.findByText(copy.error, {}, { timeout: 15_000 })).toBeInTheDocument();
    expect(
      await within(screen.getByTestId("overview-needs")).findByText(
        c.couldntCheck(c.needs.source.agents),
        {},
        { timeout: 5000 },
      ),
    ).toBeInTheDocument();
  });

  it('the Agents health filter shows the error with Retry, not "no results"', async () => {
    server.use(
      http.get("*/api/agents", ({ request }) =>
        new URL(request.url).searchParams.get("limit") === "100"
          ? new HttpResponse("boom", { status: 500 })
          : undefined,
      ),
    );
    renderApp("/agents?health=watch");
    expect(
      await screen.findByRole("button", { name: /retry/i }, { timeout: 5000 }),
    ).toBeInTheDocument();
    expect(screen.queryByText(/No agents match/i)).toBeNull();
  });
});

describe("first run (design 7A)", () => {
  it("leads with the Setup guide card instead of the deploy card, and it reopens the guide", async () => {
    server.use(http.get("*/api/agents", () => HttpResponse.json([])));
    const user = userEvent.setup();
    renderApp("/");
    const guideCard = await screen.findByTestId("overview-setup-guide");
    expect(document.querySelector('[data-testid="overview-first-run"]')).toBeNull();
    // One Deploy (the first run's: the header's steps aside), and the header's Setup guide steps aside for the card.
    expect(screen.getAllByRole("link", { name: deployCopy.entry.label })).toHaveLength(1);
    expect(screen.queryByRole("button", { name: c.setup.button })).toBeNull();
    await user.click(within(guideCard).getByRole("button", { name: "Resume guide" }));
    expect(
      await screen.findByRole("heading", { name: "Deploy your first agent" }),
    ).toBeInTheDocument();
  });

  it("opens the CLI path with every command, and each guide row at its own step", async () => {
    server.use(http.get("*/api/agents", () => HttpResponse.json([])));
    const user = userEvent.setup();
    renderApp("/");
    const guideCard = await screen.findByTestId("overview-setup-guide");
    await user.click(screen.getByRole("button", { name: c.firstRun.cli(3) }));
    expect(await screen.findByText(/^nasiko connect /)).toBeInTheDocument();
    expect(screen.getByText("nasiko deploy ./my-agent")).toBeInTheDocument();
    await user.click(within(guideCard).getByRole("button", { name: /^Connect a model/ }));
    expect(
      await screen.findByRole("heading", { name: "Connect a model provider" }),
    ).toBeInTheDocument();
  });

  it("says Start guide until a step is done", async () => {
    server.use(http.get("*/api/agents", () => HttpResponse.json([])));
    configureMocks({ onboarding: { persona: null, completed: true } });
    server.use(http.get("*/api/llm-configs", () => HttpResponse.json({ data: [] })));
    renderApp("/");
    const guideCard = await screen.findByTestId("overview-setup-guide");
    expect(
      await within(guideCard).findByRole("button", { name: "Start guide" }),
    ).toBeInTheDocument();
  });

  it("previews what a deploy brings, without numbers", async () => {
    server.use(http.get("*/api/agents", () => HttpResponse.json([])));
    renderApp("/");
    const preview = await screen.findByTestId("overview-preview");
    expect(
      within(preview)
        .getAllByRole("heading", { level: 3 })
        .map((h) => h.textContent),
    ).toEqual([c.needs.title, c.spend.title, c.health.title]);
    expect(preview.textContent).not.toMatch(/\d/);
  });

  it("opens the guide from the header Setup guide", async () => {
    const user = userEvent.setup();
    renderApp("/?scope=yours");
    const button = await screen.findByRole("button", { name: "Setup guide" });
    await waitFor(() => expect(button).toBeEnabled());
    await user.click(button);
    expect(
      await screen.findByRole("dialog", { name: "Your workspace is ready" }),
    ).toBeInTheDocument();
  });

  it("shows the deploy card and reads no money or sessions with an empty fleet on an older server", async () => {
    configureMocks({ variant: "onboarding-absent" });
    server.use(http.get("*/api/agents", () => HttpResponse.json([])));
    const rec = recordRequests();
    renderApp("/");
    expect(
      await screen.findByRole("heading", { level: 2, name: c.firstRun.title }),
    ).toBeInTheDocument();
    expect(document.querySelector('[data-testid="overview-needs"]')).toBeNull();
    await new Promise((r) => setTimeout(r, 300));
    rec.stop();
    const paths = rec.urls.map((u) => u.pathname);
    expect(paths.filter((p) => p.includes("/finops/"))).toEqual([]);
    expect(paths.filter((p) => p.includes("/session/"))).toEqual([]);
  });
});

describe("a harness-only fleet (review)", () => {
  it("settles Needs you and the counts instead of loading forever", async () => {
    const page = (await (await fetch("/api/agents?limit=100&offset=0")).json()) as {
      tags?: string[];
      owner_id: string;
    }[];
    const me = (await (await fetch("/api/me")).json()) as { sub: string };
    const harnesses = page.filter((a) => a.tags?.includes("coding-agent") && a.owner_id === me.sub);
    server.use(
      http.get("/api/agents", ({ request }) =>
        HttpResponse.json(
          Number(new URL(request.url).searchParams.get("offset") ?? 0) ? [] : harnesses,
        ),
      ),
    );
    renderApp("/?scope=yours");
    await table();
    await waitFor(() => expect(within(needs()).queryByLabelText(c.loading)).toBeNull(), {
      timeout: 8000,
    });
    await waitFor(async () => expect(await needsTile()).not.toHaveTextContent("—"), {
      timeout: 8000,
    });
  });
});
