import { act, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ExpressiveRegion, MotionProvider, clampMode, useMotion } from "./provider";
import { usePortalContainer } from "./portal";

function Probe({ id }: { id: string }) {
  const m = useMotion();
  const c = usePortalContainer();
  return (
    <span
      data-testid={id}
      data-mode={m.mode}
      data-profile={m.profile}
      data-container={c ? c.getAttribute("data-motion") : "body"}
    >
      {m.mode}
    </span>
  );
}

describe("clampMode", () => {
  it("never exceeds the parent", () => {
    expect(clampMode("full", "full")).toBe("full");
    expect(clampMode("reduced", "full")).toBe("reduced");
    expect(clampMode("none", "full")).toBe("none");
    expect(clampMode("full", "none")).toBe("none");
    expect(clampMode("reduced", "none")).toBe("none");
  });
});

describe("MotionProvider ownership", () => {
  it("the app scope owns html[data-motion]; surfaces do not touch it", () => {
    const view = render(
      <MotionProvider scope="app" preference="reduced" osReducedOverride={false}>
        <MotionProvider preference="off">
          <Probe id="s" />
        </MotionProvider>
      </MotionProvider>,
    );
    expect(document.documentElement.dataset.motion).toBe("reduced");
    expect(screen.getByTestId("s")).toHaveAttribute("data-mode", "none");
    view.unmount();
    expect(document.documentElement.dataset.motion).toBeUndefined();
  });

  it("a surface cannot exceed the application/OS ceiling", () => {
    render(
      <MotionProvider scope="app" preference="reduced" osReducedOverride={false}>
        <MotionProvider preference="system" profile="expressive">
          <Probe id="s" />
        </MotionProvider>
      </MotionProvider>,
    );
    expect(screen.getByTestId("s")).toHaveAttribute("data-mode", "reduced");
  });

  it("a surface can lower its own policy below the app", () => {
    render(
      <MotionProvider scope="app" preference="system" osReducedOverride={false}>
        <MotionProvider preference="reduced">
          <Probe id="s" />
        </MotionProvider>
      </MotionProvider>,
    );
    expect(document.documentElement.dataset.motion).toBe("full");
    expect(screen.getByTestId("s")).toHaveAttribute("data-mode", "reduced");
  });

  it("sibling surfaces are independent, and unmounting one leaves the other and the app untouched", () => {
    function Host({ showA }: { showA: boolean }) {
      return (
        <MotionProvider scope="app" preference="system" osReducedOverride={false}>
          {showA ? (
            <MotionProvider preference="off">
              <Probe id="a" />
            </MotionProvider>
          ) : null}
          <MotionProvider preference="system">
            <Probe id="b" />
          </MotionProvider>
        </MotionProvider>
      );
    }
    const view = render(<Host showA />);
    expect(screen.getByTestId("a")).toHaveAttribute("data-mode", "none");
    expect(screen.getByTestId("b")).toHaveAttribute("data-mode", "full");
    expect(document.documentElement.dataset.motion).toBe("full");
    act(() => {
      view.rerender(<Host showA={false} />);
    });
    expect(screen.queryByTestId("a")).toBeNull();
    expect(screen.getByTestId("b")).toHaveAttribute("data-mode", "full");
    expect(document.documentElement.dataset.motion).toBe("full");
  });

  it("each surface provides its own portal container carrying its effective policy; the app scope portals to body", () => {
    render(
      <MotionProvider scope="app" preference="system" osReducedOverride={false}>
        <Probe id="app" />
        <MotionProvider preference="off">
          <Probe id="s" />
        </MotionProvider>
      </MotionProvider>,
    );
    expect(screen.getByTestId("app")).toHaveAttribute("data-container", "body");
    expect(screen.getByTestId("s")).toHaveAttribute("data-container", "none");
    expect(document.querySelectorAll("[data-portal-root]")).toHaveLength(1);
  });

  it("ExpressiveRegion raises the profile but is clamped by its parent surface", () => {
    render(
      <MotionProvider scope="app" preference="system" osReducedOverride={false}>
        <MotionProvider preference="reduced">
          <ExpressiveRegion>
            <Probe id="e" />
          </ExpressiveRegion>
        </MotionProvider>
      </MotionProvider>,
    );
    const e = screen.getByTestId("e");
    expect(e).toHaveAttribute("data-profile", "expressive");
    expect(e).toHaveAttribute("data-mode", "reduced");
  });

  it("nested surfaces clamp transitively (app → surface → surface)", () => {
    render(
      <MotionProvider scope="app" preference="system" osReducedOverride={false}>
        <MotionProvider preference="reduced">
          <MotionProvider preference="system" profile="expressive">
            <Probe id="n" />
          </MotionProvider>
        </MotionProvider>
      </MotionProvider>,
    );
    expect(screen.getByTestId("n")).toHaveAttribute("data-mode", "reduced");
  });
});
