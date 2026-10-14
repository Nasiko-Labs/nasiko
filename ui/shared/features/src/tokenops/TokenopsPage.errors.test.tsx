// Ported from nasiko-ui-lab@1b99bdda common/src/features/tokenops/TokenopsPage.errors.test.tsx. Local edits: `renderApp` -> `renderTokenops`, `router.state.location` ->
// `location()`; `server` from `@/test/pinned-seed`; the 401-burst case asserts the query client's `onUnauthorized`
// (the harness renders the page, not the shell that turns it into the /login redirect); Prettier formatting.
/* eslint-disable @typescript-eslint/no-misused-spread -- the lab's test code: the seed's shape and the DOM it asserts on are what the test checks */
/**
 * TokenOps error states: the session and per-panel errors with Retry, the unknown or inaccessible agent filter
 * (a notice, not six error panels), the traces drawer's access 404, a burst of 401s, and the server-down page's reload.
 * Same setup as TokenopsPage.test.tsx: pinned time, seed-backed MSW handlers, the real router.
 */
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { describe, expect, it, vi } from "vitest";
import { configureMocks } from "@nasiko/mocks/handlers";
import { seed, section, server, setupPinnedSeed } from "@nasiko/testing/pinned-seed";
import { location, renderTokenops } from "./test-utils";

const FINOPS = "/api/observability/finops";

setupPinnedSeed();

describe("TokenOps page: error states", () => {
  it("/api/me 500 shows a session error panel whose Retry recovers the page", async () => {
    const user = userEvent.setup();
    server.use(http.get("/api/me", () => new HttpResponse("internal error", { status: 500 })));
    renderTokenops("/tokenops?open=all");
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Couldn't load your session");
    expect(alert).toHaveTextContent("internal error");
    server.resetHandlers();
    await user.click(within(alert).getByRole("button", { name: /Retry/ }));
    expect(await screen.findByTestId("mtd-spend")).toBeInTheDocument();
  });

  it.each([
    [
      "timeseries",
      `${FINOPS}/spend-timeseries`,
      "Couldn't load spend over time",
      "/tokenops?open=all",
    ],
    [
      "calendar",
      `${FINOPS}/spend-calendar`,
      "Couldn't load this month's spend",
      "/tokenops?open=all",
    ],
    [
      "day drill-down",
      `${FINOPS}/spend-calendar/day`,
      "Couldn't load the hourly breakdown",
      `/tokenops?open=all&day=${seed.spikeDate}`,
    ],
  ])("%s 500: only that panel errors, and Retry recovers it", async (_, path, message, url) => {
    const user = userEvent.setup();
    server.use(http.get(path, () => new HttpResponse("internal error", { status: 500 })));
    renderTokenops(url);
    const alert = (await screen.findAllByRole("alert")).find((a) =>
      a.textContent?.includes(message),
    )!;
    expect(alert).toBeTruthy();
    expect(await screen.findByLabelText(/^Key metrics/)).toBeInTheDocument();
    server.resetHandlers();
    await user.click(within(alert).getByRole("button", { name: /Retry/ }));
    await waitFor(() => {
      expect(screen.queryByText(message)).toBeNull();
    });
  });

  it("Retry in the F3 panel and in the traces drawer refetches and recovers", async () => {
    const user = userEvent.setup();
    server.use(
      http.get(`${FINOPS}/dashboard`, () => new HttpResponse("internal error", { status: 500 })),
      http.get(`${FINOPS}/top-traces`, () => new HttpResponse("internal error", { status: 500 })),
    );
    renderTokenops("/tokenops?open=all&traces=true");
    const drawer = await screen.findByRole("dialog");
    const drawerRetry = await within(drawer).findByRole("button", { name: /Retry/ });
    server.resetHandlers();
    await user.click(drawerRetry);
    expect(
      (await within(drawer).findAllByRole("link", { name: /^Open trace:/ })).length,
    ).toBeGreaterThan(0);
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    const attribution = await section("Who is driving cost");
    await user.click(within(attribution).getByRole("button", { name: /Retry/ }));
    expect(await within(attribution).findByRole("table")).toBeInTheDocument();
  });

  it('a top-traces 404 "agent not found" is an access answer, not a missing endpoint', async () => {
    server.use(
      http.get(`${FINOPS}/top-traces`, () => new HttpResponse("agent not found", { status: 404 })),
    );
    renderTokenops("/tokenops?open=all&traces=true");
    const drawer = await screen.findByRole("dialog");
    expect(await within(drawer).findByRole("button", { name: /Retry/ })).toBeInTheDocument();
    expect(within(drawer).queryByText(/newer nasiko-server/)).toBeNull();
  });

  it("several 401s at once ask for the /login redirect once", async () => {
    const { queryClient, onUnauthorized } = renderTokenops("/tokenops?open=all&preset=7d");
    await screen.findByTestId("mtd-spend");
    configureMocks({ loggedIn: false });
    // The shell's redirect leaves the page; here it stays mounted, so the reaction is held pending as the
    // navigation would be, and a second burst from the re-registered observers cannot ask again.
    onUnauthorized.mockImplementation(() => new Promise<void>(() => undefined));
    // Every active query refetches at once and gets a 401.
    await act(async () => {
      await queryClient.invalidateQueries();
    });
    await waitFor(() => {
      expect(onUnauthorized).toHaveBeenCalled();
    });
    expect(onUnauthorized).toHaveBeenCalledTimes(1);
  });
});

// Regression: ISSUE-006 (/qa 2026-09-26, .gstack/qa-reports/qa-report-localhost-3000-2026-09-26.md): an unknown
// agent filter rendered six error panels with futile Retry buttons.
describe("unknown agent filter", () => {
  it("400: only the notice, no error panels or Retry; clearing restores the page", async () => {
    const user = userEvent.setup();
    renderTokenops("/tokenops?open=all&agent=nope");
    expect(await screen.findByText(/unknown agent 'nope'/)).toBeInTheDocument();
    expect(screen.queryByText(/Couldn't load/)).toBeNull();
    expect(screen.queryByRole("button", { name: /Retry/ })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Clear agent filter" }));
    await waitFor(() => {
      expect(location().search).not.toHaveProperty("agent");
    });
    expect(await screen.findByRole("heading", { name: "Who is driving cost" })).toBeInTheDocument();
  });

  it('404 "agent not found" (no access): same treatment', async () => {
    const id = seed.agents.find((a) => !a.deleted)!.id;
    server.use(
      http.get("/api/observability/finops/dashboard", ({ request }) =>
        new URL(request.url).searchParams.get("agent_id")
          ? new HttpResponse("agent not found", { status: 404 })
          : undefined,
      ),
    );
    renderTokenops(`/tokenops?open=all&agent=${id}`);
    expect(await screen.findByRole("button", { name: "Clear agent filter" })).toBeInTheDocument();
    expect(screen.queryByText(/Couldn't load/)).toBeNull();
  });

  it("other dashboard failures still show panel errors with Retry", async () => {
    server.use(
      http.get(
        "/api/observability/finops/dashboard",
        () => new HttpResponse("internal error", { status: 500 }),
      ),
    );
    renderTokenops("/tokenops?open=all");
    expect((await screen.findAllByRole("button", { name: /Retry/ })).length).toBeGreaterThan(0);
  });
});

describe("server down", () => {
  it('"Try again" reloads the page', async () => {
    const reload = vi.fn();
    const real = window.location;
    vi.stubGlobal("location", { ...real, origin: real.origin, href: real.href, reload });
    const user = userEvent.setup();
    server.use(http.get("/api/me", () => new HttpResponse("bad gateway", { status: 502 })));
    renderTokenops("/tokenops?open=all");
    await user.click(await screen.findByRole("button", { name: "Try again" }));
    expect(reload).toHaveBeenCalledTimes(1);
  });
});
