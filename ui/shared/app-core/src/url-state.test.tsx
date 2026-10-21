import { act, screen } from "@testing-library/react";
import { useEffect } from "react";
import { describe, expect, it, vi } from "vitest";
import * as z from "zod/mini";
import { renderWithRouter, routerState } from "@nasiko/testing/router";
import { historyIndex, useSearchState } from "./url-state";

const schema = z.object({
  preset: z._default(z.catch(z.enum(["30d", "7d"]), "30d"), "30d"),
  day: z.optional(z.string()),
});
type Search = z.infer<typeof schema>;

/** The hook's latest result, for the test to call (written in an effect, never during render). */
const probe: { api: ReturnType<typeof useSearchState<Search>> | null } = { api: null };
function Probe() {
  const state = useSearchState(schema);
  useEffect(() => {
    probe.api = state;
  });
  return <output data-testid="search">{JSON.stringify(state.search)}</output>;
}

describe("useSearchState", () => {
  it("a search change uses the client router, push vs replace without scrolling", () => {
    const { router } = renderWithRouter(<Probe />, { url: "/tokenops" });
    const push = vi.spyOn(router, "push");
    const replace = vi.spyOn(router, "replace");
    const start = routerState().length;

    act(() => {
      probe.api?.setSearch({ day: "2026-10-05" });
    });
    expect(routerState().href).toBe("/tokenops?day=2026-10-05");
    expect(routerState().length).toBe(start + 1);
    expect(screen.getByTestId("search").textContent).toContain("2026-10-05");

    act(() => {
      probe.api?.setSearch({ preset: "7d" }, { replace: true });
    });
    expect(routerState().href).toBe("/tokenops?preset=7d&day=2026-10-05");
    expect(routerState().length).toBe(start + 1);

    // Neither went through a router navigation: the History API only.
    expect(push).toHaveBeenCalled();
    expect(replace).toHaveBeenCalled();
  });

  it("defaults stay out of the URL", () => {
    renderWithRouter(<Probe />, { url: "/tokenops?preset=7d" });
    act(() => {
      probe.api?.setSearch({ preset: "30d" });
    });
    expect(routerState().href).toBe("/tokenops");
  });

  it("stamps a history index that Back restores (close-goes-Back depends on it)", () => {
    renderWithRouter(<Probe />, { url: "/tokenops" });
    // In the browser the router stamps the landing entry's state (`__TSR_index`) when it mounts; the harness starts
    // each test on a fresh, unstamped entry, so do the same.
    window.history.replaceState(window.history.state, "");
    const before = historyIndex();
    act(() => {
      probe.api?.setSearch({ day: "2026-10-05" });
    });
    const opened = historyIndex();
    expect(opened).toBe(before + 1);
    act(() => {
      probe.api?.setSearch({ preset: "7d" }, { replace: true });
    });
    expect(historyIndex()).toBe(opened);
    act(() => {
      probe.api?.back();
    });
    expect(historyIndex()).toBe(before);
    expect(routerState().href).toBe("/tokenops");
  });

  // Ported from the legacy ui/ on development (3aac84f66) common/src/lib/search.test.tsx.
  it("keeps the sibling list across a search change and drops other state", () => {
    renderWithRouter(<Probe />, { url: "/agents/a" });
    act(() => {
      window.history.replaceState(
        { siblings: ["a", "b"], agentDeleted: { name: "x" } },
        "",
        "/agents/a",
      );
    });
    act(() => {
      probe.api?.setSearch({ day: "2026-10-05" });
    });
    expect(routerState().href).toBe("/agents/a?day=2026-10-05");
    expect(window.history.state).toMatchObject({ siblings: ["a", "b"] });
    expect(window.history.state).not.toHaveProperty("agentDeleted");
  });
});
