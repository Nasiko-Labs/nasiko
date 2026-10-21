// Ported from nasiko-ui-lab@1b99bdda common/src/features/router/RouterPage.live.test.tsx. Local edits: before clicking Change routing the
// test waits for every row's routing read to settle (RoutingSheet.test.tsx's `ready()`): until then a settling read
// can move Doc Writer's row into the folded "on your default" body, which remounts its button, and a click on the
// detached one opens nothing (the lab's run won that race by timing); imports; Prettier formatting.
// Legacy ui/ on development (3aac84f66, c721a11a8): the summary is the Agents table's filter tabs.
/**
 * The router page in live mode with router, agents and providers partially mocked (VITE_NASIKO_MOCK): the live user
 * (a real `me.sub` no seed row uses) maps onto the seed admin (N19), so "Your agents" lists the seed agents, and
 * `?mock=` variants are ignored (they would mix seed and real-server states).
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { configureMocks } from "@nasiko/mocks/handlers";
import { now, seed, setupPinnedSeed } from "@nasiko/testing/pinned-seed";
import { renderApp } from "@nasiko/app-shell/testing/render-app";
import { server } from "@nasiko/testing/pinned-seed";
import { copy } from "./copy";

vi.mock("@nasiko/app-core/env", async (orig) => {
  const mod = await orig<typeof import("@nasiko/app-core/env")>();
  return {
    ...mod,
    env: { ...mod.env, mode: "live", partialMocks: ["router", "agents", "providers"] },
  };
});

setupPinnedSeed();
// These tests act on agents that follow the default: show that group (it folds by default).
beforeEach(() => {
  localStorage.setItem("openruntime.router.defaultsOpen", "open");
});
afterEach(() => {
  configureMocks({ seed, now, loggedIn: true, lockPersona: false });
});

const REAL_SUB = "0b0b0b0b-0000-4000-8000-000000000001";

describe("partial mocks in live mode", () => {
  it("lists the seed admin’s agents for the live user and ignores ?mock=", async () => {
    configureMocks({ lockPersona: true });
    server.use(
      http.get("/api/me", () =>
        HttpResponse.json({ sub: REAL_SUB, username: "admin", is_superuser: true }),
      ),
    );
    window.history.replaceState(null, "", "/router?mock=router-empty");
    try {
      renderApp("/router");
      await screen.findByRole("table", { name: copy.agentsTitle });
      await waitFor(() =>
        expect(
          within(screen.getByRole("tablist", { name: copy.summaryLabel })).getByRole("tab", {
            name: "19 agents",
          }),
        ).toBeInTheDocument(),
      );
      expect(screen.queryByText(copy.noConfigs)).toBeNull();
      // Every row read settled, so the row (and its button) stays where it is while clicking.
      await waitFor(() => {
        expect(screen.queryAllByText(copy.readingRouting)).toHaveLength(0);
      });
      // The live user owns these rows, so the sheet offers their own configs.
      await userEvent.click(
        screen.getByRole("button", { name: `${copy.changeRouting}: Doc Writer` }),
      );
      const sheet = await screen.findByRole("dialog", { name: copy.routingTitle("Doc Writer") });
      expect(within(sheet).getByRole("radio", { name: copy.useMyDefault })).toBeChecked();
      expect(
        within(sheet).getByRole("radio", { name: new RegExp(`^${copy.useAConfig}`) }),
      ).toBeEnabled();
    } finally {
      window.history.replaceState(null, "", "/");
    }
  });
});
