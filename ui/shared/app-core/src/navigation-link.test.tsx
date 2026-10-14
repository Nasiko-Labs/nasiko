// Destination test: the navigation port's Link preloads its route on intent (the lab's `defaultPreload: "intent"`),
// never on render or viewport entry.
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HOVER_DELAY_MS, Link, NavigationContext, type Navigation } from "./navigation";

function setup(href = "/agents?q=a") {
  const router: Navigation = {
    push: vi.fn(),
    replace: vi.fn(),
    back: vi.fn(),
    forward: vi.fn(),
    refresh: vi.fn(),
    prefetch: vi.fn(),
  };
  render(
    <NavigationContext value={{ router, pathname: "/", search: "" }}>
      <Link href={href}>Agents</Link>
      <Link href="https://example.com/x">Elsewhere</Link>
      <Link href="/report.csv" download>
        Report
      </Link>
    </NavigationContext>,
  );
  return router;
}

describe("Link intent preload", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("preloads nothing on render", () => {
    const router = setup();
    expect(router.prefetch).not.toHaveBeenCalled();
  });

  it("preloads after a mouse rests on the link, not when it passes over", () => {
    const router = setup();
    const link = screen.getByRole("link", { name: "Agents" });
    fireEvent.pointerEnter(link, { pointerType: "mouse" });
    act(() => {
      vi.advanceTimersByTime(HOVER_DELAY_MS - 10);
    });
    fireEvent.pointerLeave(link, { pointerType: "mouse" });
    act(() => {
      vi.advanceTimersByTime(HOVER_DELAY_MS);
    });
    expect(router.prefetch).not.toHaveBeenCalled();
    fireEvent.pointerEnter(link, { pointerType: "mouse" });
    act(() => {
      vi.advanceTimersByTime(HOVER_DELAY_MS);
    });
    expect(router.prefetch).toHaveBeenCalledWith("/agents?q=a");
  });

  it("preloads at once on keyboard focus and on touch", () => {
    const router = setup();
    const link = screen.getByRole("link", { name: "Agents" });
    fireEvent.focus(link);
    expect(router.prefetch).toHaveBeenCalledTimes(1);
    fireEvent.pointerDown(link, { pointerType: "touch" });
    expect(router.prefetch).toHaveBeenCalledTimes(2);
  });

  it("never preloads a link that leaves the app or downloads", () => {
    const router = setup();
    fireEvent.focus(screen.getByRole("link", { name: "Elsewhere" }));
    fireEvent.focus(screen.getByRole("link", { name: "Report" }));
    expect(router.prefetch).not.toHaveBeenCalled();
  });

  it("a plain click navigates through the port", () => {
    const router = setup();
    fireEvent.click(screen.getByRole("link", { name: "Agents" }));
    expect(router.push).toHaveBeenCalledWith("/agents?q=a", undefined);
  });
});
