// Ported from nasiko-ui-lab@1b99bdda common/src/features/tokenops/TokenopsSummary.test.tsx. Local edits: `renderApp` -> `renderTokenops`, `router.state.location` ->
// `location()` (its `index` is the lab's `__TSR_index`); the disclosure buttons are found by id
// (`disclosure()`), since Base UI's time-control toggles are buttons named "This month" too; Prettier formatting.
/* eslint-disable @typescript-eslint/unbound-method -- the lab's test code: the seed's shape and the DOM it asserts on are what the test checks */
import { screen, within, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { disclosure, FIXED, seed, setupPinnedSeed } from "@nasiko/testing/pinned-seed";
import { location, renderTokenops } from "./test-utils";

setupPinnedSeed();

describe("TokenOps summary", () => {
  it("leads with the narrative; Spend and Token optimisation start open, the rest are collapsed", async () => {
    renderTokenops("/tokenops");
    expect(await screen.findByTestId("summary-narrative")).toHaveTextContent(
      /^In the last 30 days you spent at least \$[\d.,]+, \d+% (more|less) than the period before\. Code Reviewer drove \d+% of it\.$/,
    );
    // Savings are the one piece of good news on this page. Collapsed by default, they were
    // invisible to anyone who did not already know to look for them.
    for (const id of ["spend", "optimise"]) {
      expect(disclosure(id)).toHaveAttribute("aria-expanded", "true");
    }
    for (const id of ["drivers", "month", "metrics"]) {
      expect(disclosure(id)).toHaveAttribute("aria-expanded", "false");
    }
    expect(screen.getByRole("group", { name: "Month progress" })).toHaveTextContent(
      /spent this month/,
    );
  });

  it("lands on the optimisation section when opened by link, not at the top of the page", async () => {
    // The regression this guards: the section expanded but the viewport never moved, because the
    // scroll ran before the disclosures had painted and nothing made it try again.
    //
    // Records `this.id` rather than using the spy's `mock.instances`, which is populated for `new`
    // calls and stays empty for a patched prototype method — asserting on it passes vacuously.
    const scrolled: string[] = [];
    const original = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push(this.id);
    };
    try {
      renderTokenops("/tokenops?open=optimise");
      await screen.findByRole("button", { name: /^Token optimisation/ });
      await waitFor(() => {
        expect(scrolled).toContain("disclosure-optimise-title");
      });
    } finally {
      Element.prototype.scrollIntoView = original;
    }
  });

  it("the in-page button lands exactly where the Overview link does", async () => {
    // The user-visible bug: two ways in, two landing positions. The button scrolled without the
    // sticky-bar offset, so it stopped with the heading tucked under the bar.
    const offsets: (string | undefined)[] = [];
    const original = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function (this: Element) {
      if (this.id === "disclosure-optimise-title") {
        offsets.push((this as HTMLElement).style.scrollMarginTop);
      }
    };
    try {
      renderTokenops("/tokenops");
      const banner = await screen.findByRole("region", { name: /Optimisation saved you/ });
      await userEvent.click(within(banner).getByRole("button", { name: /See the breakdown/ }));
      await waitFor(() => {
        expect(offsets).toHaveLength(1);
      });
      // Offset applied, not a bare scroll: that is what keeps the heading clear of the sticky bar.
      expect(offsets[0]).toMatch(/^\d+px$/);
    } finally {
      Element.prototype.scrollIntoView = original;
    }
  });

  it("puts the savings headline above the fold, before any section is expanded", async () => {
    renderTokenops("/tokenops");
    const banner = await screen.findByRole("region", { name: /Optimisation saved you/ });
    expect(banner).toHaveTextContent(/fewer tokens/);
    expect(within(banner).getByRole("button", { name: /See the breakdown/ })).toBeInTheDocument();
  });

  it("a disclosure opens in place and records itself in ?open (replace)", async () => {
    const user = userEvent.setup();
    renderTokenops("/tokenops");
    const before = location().index;
    await user.click(await screen.findByRole("button", { name: /^Who is driving cost/ }));
    await waitFor(() => {
      expect(location().search).toMatchObject({ open: "drivers" });
    });
    expect(location().index).toBe(before);
    expect(
      (await screen.findAllByRole("region", { name: /^Who is driving cost/ })).length,
    ).toBeGreaterThan(0);
  });

  it("opening one disclosure collapses the others", async () => {
    const user = userEvent.setup();
    renderTokenops("/tokenops");
    const expanded = (name: string) =>
      screen.getByRole("button", { name: new RegExp(`^${name}`) }).getAttribute("aria-expanded");
    await user.click(await screen.findByRole("button", { name: /^Who is driving cost/ }));
    await waitFor(() => {
      expect(expanded("Who is driving cost")).toBe("true");
    });
    expect(expanded("Spend over time")).toBe("false");
    await user.click(screen.getByRole("button", { name: /^All metrics/ }));
    await waitFor(() => {
      expect(expanded("All metrics")).toBe("true");
    });
    expect(expanded("Who is driving cost")).toBe("false");
    expect(expanded("Spend over time")).toBe("false");
  });

  it("This month also opens Spend over time on today's hour-by-hour breakdown", async () => {
    const user = userEvent.setup();
    const scrolled = vi.spyOn(Element.prototype, "scrollIntoView");
    renderTokenops("/tokenops?open=drivers");
    await screen.findByRole("button", { name: /^Who is driving cost/ });
    await user.click(disclosure("month"));
    await waitFor(() => {
      expect(scrolled.mock.contexts).toContain(disclosure("month"));
    });
    const today = FIXED.toISOString().slice(0, 10);
    await waitFor(() => {
      expect(location().search).toMatchObject({ open: "spend,month", day: today });
    });
    expect(screen.getByRole("button", { name: /^Spend over time/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(screen.getByRole("button", { name: /^Who is driving cost/ })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
    expect(
      await screen.findByRole("heading", { name: /Mar 20, 2026 · hour by hour/ }),
    ).toBeInTheDocument();
    scrolled.mockRestore();
  });

  it("a day drill-down auto-opens Spend over time and offers that day's sessions", async () => {
    renderTokenops(`/tokenops?day=${seed.spikeDate}`);
    expect(await screen.findByRole("button", { name: /^Spend over time/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    const links = await screen.findAllByRole("link", { name: /See sessions/ });
    expect(links.some((l) => l.getAttribute("href")?.includes(`day=${seed.spikeDate}`))).toBe(true);
  });

  it("Compare off: no change clause, no Δ in the figures or KPIs", async () => {
    renderTokenops("/tokenops?compare=0&open=metrics");
    await waitFor(() =>
      expect(screen.getByTestId("summary-narrative")).toHaveTextContent(
        /^In the last 30 days you spent at least \$[\d.,]+\. /,
      ),
    );
    expect(screen.getByText("Compare is off")).toBeInTheDocument();
    expect(
      (await screen.findAllByLabelText("comparison unavailable")).length,
    ).toBeGreaterThanOrEqual(4);
  });
});
