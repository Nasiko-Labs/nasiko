// Ported from nasiko-ui-lab@1b99bdda common/src/features/router/RoutingSheet.test.tsx (its "agent detail card" cases,
// which need only the agent detail page; the Router-page cases stay in that file for the Router port). Local edits:
// the four cases and the helpers they use are split into this file; the disabled config radio is checked by
// `aria-disabled` (Base UI's radio is a span); imports (\`renderApp\` is
// @nasiko/app-shell/testing/render-app; the pinned seed, msw server and request recorder are
// @nasiko/testing/pinned-seed); Prettier formatting.

/**
 * The agent detail's routing card (plans/feat-llm-router.md §4.5, §8): the owner gate on the agent page, superuser
 * labels, and a 403 that hides the card.
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
  configureMocks({ seed, now, loggedIn: true, superuser: null, routerVariants: [] });
});

const agentNamed = (display: string) => seed.agents.find((a) => a.display_name === display)!;

describe("agent detail card", () => {
  it("the owner sees the card and can change routing from it", async () => {
    renderApp(`/agents/${agentNamed("Research Agent").id}`);
    const card = (await screen.findByRole("heading", { name: copy.cardTitle })).closest("section")!;
    await waitFor(() => expect(card).toHaveTextContent(copy.overridden("gpt-4o")));
    await userEvent.click(within(card).getByRole("button", { name: copy.changeRouting }));
    expect(
      await screen.findByRole("dialog", { name: copy.routingTitle("Research Agent") }),
    ).toBeInTheDocument();
  });

  it("a non-owner who is not a superuser never reads the routing", async () => {
    configureMocks({ superuser: false });
    const other = seed.agents[13]!;
    const calls = recordRequests();
    renderApp(`/agents/${other.id}`);
    await screen.findByRole("heading", { level: 1 });
    await screen.findByRole("tab", { name: "Overview" });
    expect(screen.queryByRole("heading", { name: copy.cardTitle })).toBeNull();
    expect(calls.urls.filter((u) => u.pathname.endsWith("/llm-config"))).toEqual([]);
    calls.stop();
  });

  it("a superuser on another owner’s agent sees owner labels and can’t pick a config", async () => {
    const other = seed.agents[13]!;
    renderApp(`/agents/${other.id}`);
    const card = (await screen.findByRole("heading", { name: copy.cardTitle })).closest("section")!;
    await waitFor(() => expect(card).toHaveTextContent(copy.source.ownerDefault));
    await userEvent.click(within(card).getByRole("button", { name: copy.changeRouting }));
    const sheet = await screen.findByRole("dialog", {
      name: copy.routingTitle(other.display_name),
    });
    expect(within(sheet).getByRole("radio", { name: copy.useOwnersDefault })).toBeChecked();
    // Base UI's radio is a `span role="radio"`: disabled is `aria-disabled` (Radix's was a disabled <button>).
    expect(
      within(sheet).getByRole("radio", { name: new RegExp(`^${copy.useAConfig}`) }),
    ).toHaveAttribute("aria-disabled", "true");
    expect(within(sheet).getByText(copy.cantSeeOwnerConfigs)).toBeInTheDocument();
  });

  it("a 403 hides the card and keeps the session", async () => {
    server.use(
      http.get(
        "/api/agents/:id/llm-config",
        () => new Response("not the agent owner", { status: 403 }),
      ),
    );
    const { router } = renderApp(`/agents/${agentNamed("Research Agent").id}`);
    await screen.findByRole("tab", { name: "Overview" });
    await waitFor(() => {
      expect(screen.queryByRole("heading", { name: copy.cardTitle })).toBeNull();
    });
    expect(router.state.location.pathname).toBe(`/agents/${agentNamed("Research Agent").id}`);
  });
});
