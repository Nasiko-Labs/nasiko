/**
 * The product tour: the top bar's tour button (or `?`) opens the page's tour; a first-time user gets one on their own
 * once the setup guide is out of the way, with the app tour folded in, and No thanks turns that off; everyone else
 * only gets tours they ask for.
 */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { copy as shellCopy } from "@nasiko/app-core/shell/copy";
import { configureMocks } from "@nasiko/mocks/handlers";
import { ADMIN_ID } from "@nasiko/mocks/seed-harness";
import { setupPinnedSeed } from "@nasiko/testing/pinned-seed";
import { renderApp } from "@nasiko/app-shell/testing/render-app";
import { copy } from "./copy";
import { parsePrefs, prefsKey } from "./logic";
import { TOURS } from "./tours";

setupPinnedSeed();
afterEach(() => vi.restoreAllMocks());

const prefs = () => parsePrefs(localStorage.getItem(prefsKey(ADMIN_ID)));
// The Overview also has its own (no bar there on desktop); jsdom applies no CSS, so both are found there.
const tourButton = async () => (await screen.findAllByRole("button", { name: shellCopy.tour }))[0]!;
const tour = (title: string) => screen.findByRole("dialog", { name: title });

/** jsdom has no layout: give every element a box, so targeted steps count as on screen. */
const withLayout = () =>
  vi
    .spyOn(HTMLElement.prototype, "getBoundingClientRect")
    .mockReturnValue(DOMRect.fromRect({ x: 20, y: 80, width: 200, height: 40 }));

describe("page tour", () => {
  it("opens this page's tour from the sidebar, steps through it and remembers it", async () => {
    const user = userEvent.setup();
    renderApp("/tokenops");
    await screen.findByRole("heading", { level: 1 });
    withLayout();
    await user.click(await tourButton());
    const dialog = await tour("TokenOps");
    const steps = TOURS.tokenops.length;
    expect(within(dialog).getByText(copy.step(1, steps))).toBeInTheDocument();
    await waitFor(() =>
      expect(within(dialog).getByRole("button", { name: copy.next })).toHaveFocus(),
    );

    await user.click(within(dialog).getByRole("button", { name: copy.next }));
    expect(await tour("Pick the window")).toBeInTheDocument();
    await user.keyboard("{ArrowRight}");
    expect(await tour("Narrow it down")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: copy.back }));
    expect(await tour("Pick the window")).toBeInTheDocument();

    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(prefs().seen).toContain("tokenops");
  });

  it("ends with Done on the last step", async () => {
    const user = userEvent.setup();
    renderApp("/builds");
    await screen.findByRole("heading", { level: 1 });
    await user.click(await tourButton());
    // No layout here: only the centred step is on screen, so it is also the last.
    const dialog = await tour("Builds");
    expect(within(dialog).queryByRole("button", { name: copy.skip })).toBeNull();
    await user.click(within(dialog).getByRole("button", { name: copy.done }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("opens with ?, but not while typing", async () => {
    const user = userEvent.setup();
    renderApp("/agents");
    await user.type(await screen.findByPlaceholderText(/search by name/i), "?");
    expect(screen.queryByRole("dialog")).toBeNull();
    await user.click(document.body);
    await user.keyboard("?");
    expect(await tour("Agents")).toBeInTheDocument();
  });

  it("carries the sidebar on the Overview's tour", async () => {
    const user = userEvent.setup();
    renderApp("/");
    await screen.findByRole("heading", { level: 1 });
    await user.click(await tourButton());
    expect(await tour(TOURS.app[0]!.title)).toBeInTheDocument();
    // Not a first run: nothing to opt out of.
    expect(screen.queryByRole("button", { name: copy.noThanks })).toBeNull();
  });

  it("never opens on its own for a returning user", async () => {
    renderApp("/tokenops");
    await screen.findByRole("heading", { level: 1 });
    await new Promise((r) => setTimeout(r, 1_200));
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("first-time user", () => {
  it("gets the app tour once the setup guide is skipped, and No thanks turns tours off", async () => {
    configureMocks({ onboarding: { persona: null, completed: false } });
    const user = userEvent.setup();
    renderApp("/");
    await user.click(await screen.findByRole("button", { name: /skip guide/i }));

    // On the Overview: the sidebar's steps lead its tour.
    const dialog = await tour(TOURS.app[0]!.title);
    expect(prefs().auto).toBe(true);
    expect(within(dialog).getByRole("button", { name: copy.start })).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: copy.noThanks }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(prefs()).toEqual({ auto: false, seen: ["app"] });
  });
});
