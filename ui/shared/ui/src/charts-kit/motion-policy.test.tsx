// The kit under the destination's motion policy (ADR-0003 amendment 2026-10-08, ADR-0006): a chart inside a
// `MotionProvider` whose resolved mode is `none` or `reduced` takes the kit's own at-rest path (the one the lab
// already had for the OS reduced-motion preference), whatever the OS says. The OS-reduced case is in
// `motion-policy-os-reduced.test.tsx`: motion caches the OS media query per module registry, so it needs its own file.
import { render } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { MotionProvider } from "../motion/provider";
import { Bar } from "./bar";
import { BarChart } from "./bar-chart";

// jsdom has no layout: give ParentSize a fixed box so the chart renders.
vi.mock("@visx/responsive", () => ({
  ParentSize: ({ children }: { children: (s: { width: number; height: number }) => ReactNode }) =>
    children({ width: 400, height: 200 }),
}));

const FILL = "rgb(1, 2, 3)";
const DATA = [
  { name: "a", v: 10 },
  { name: "b", v: 40 },
  { name: "c", v: 25 },
];

function Chart() {
  return (
    <BarChart data={DATA} margin={{ top: 0, right: 0, bottom: 0, left: 0 }}>
      <Bar dataKey="v" fill={FILL} />
    </BarChart>
  );
}

/** Heights of the drawn bars, in data order. */
function barHeights(container: HTMLElement): number[] {
  return [...container.querySelectorAll(`rect[fill="${FILL}"]`)].map((r) =>
    Number.parseFloat(r.getAttribute("height") ?? "NaN"),
  );
}

/** At rest: every bar is at its final geometry on the first render (taller value → taller bar, none at 0). */
function expectAtRest(container: HTMLElement) {
  const h = barHeights(container);
  expect(h).toHaveLength(3);
  for (const x of h) expect(x).toBeGreaterThan(0);
  const [a = 0, b = 0, c = 0] = h;
  expect(b).toBeGreaterThan(c);
  expect(c).toBeGreaterThan(a);
}

describe("chart kit × MotionProvider (OS allows motion)", () => {
  it("animates by default: bars start the grow reveal at height 0 (control)", () => {
    const { container } = render(
      <MotionProvider scope="app" osReducedOverride={false}>
        <Chart />
      </MotionProvider>,
    );
    expect(barHeights(container)).toEqual([0, 0, 0]);
  });

  it('a surface with profile="none" draws bars at rest', () => {
    const { container } = render(
      <MotionProvider scope="app" osReducedOverride={false}>
        <MotionProvider profile="none">
          <Chart />
        </MotionProvider>
      </MotionProvider>,
    );
    expectAtRest(container);
  });

  it("a surface whose parent is `none` is clamped, even when it asks for more", () => {
    const { container } = render(
      <MotionProvider scope="app" preference="off" osReducedOverride={false}>
        <MotionProvider profile="expressive" preference="system">
          <Chart />
        </MotionProvider>
      </MotionProvider>,
    );
    expectAtRest(container);
  });

  it("a `reduced` user preference draws bars at rest", () => {
    const { container } = render(
      <MotionProvider scope="app" preference="reduced" osReducedOverride={false}>
        <Chart />
      </MotionProvider>,
    );
    expectAtRest(container);
  });

  it("outside any provider the kit keeps the lab's rule (OS only): it animates", () => {
    const { container } = render(<Chart />);
    expect(barHeights(container)).toEqual([0, 0, 0]);
  });
});
