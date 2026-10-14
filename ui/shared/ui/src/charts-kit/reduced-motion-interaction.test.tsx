// Destination test (pilot finding "reduced-motion click blocking"): under reduced motion nothing animates, so the chart
// must be interactive at once. The kit (and the lab) kept the reveal timer as an interaction gate even then, so a
// reduced-motion user's clicks on a chart were ignored for 1.1 s after every load or range change.
import { act, render } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MotionProvider } from "../motion/provider";
import { Bar } from "./bar";
import { BarChart } from "./bar-chart";
import { Line } from "./line";
import { LineChart } from "./line-chart";

vi.mock("@visx/responsive", () => ({
  ParentSize: ({ children }: { children: (s: { width: number; height: number }) => ReactNode }) =>
    children({ width: 400, height: 200 }),
}));

const BARS = [
  { name: "a", v: 10 },
  { name: "b", v: 40 },
  { name: "c", v: 25 },
];
const DAY = 86_400_000;
const POINTS = [0, 1, 2, 3].map((i) => ({
  date: new Date(Date.UTC(2026, 9, 1) + i * DAY),
  v: i * 10 + 5,
}));

function wrap(mode: "reduced" | "full", node: ReactNode) {
  return mode === "reduced" ? (
    <MotionProvider scope="app" osReducedOverride={true}>
      {node}
    </MotionProvider>
  ) : (
    <MotionProvider scope="app" osReducedOverride={false}>
      {node}
    </MotionProvider>
  );
}

beforeEach(() => {
  vi.useFakeTimers();
  // jsdom has no SVG geometry; the line's path length only sizes its stroke dash.
  Object.defineProperty(SVGElement.prototype, "getTotalLength", {
    configurable: true,
    value: () => 100,
  });
});
afterEach(() => {
  vi.useRealTimers();
});

describe("charts are interactive at once under reduced motion", () => {
  it("bar chart: ready without waiting for the reveal timer", () => {
    const phases: string[] = [];
    render(
      wrap(
        "reduced",
        <BarChart data={BARS} onPhaseChange={(p) => phases.push(p)}>
          <Bar dataKey="v" />
        </BarChart>,
      ),
    );
    expect(phases.at(-1)).toBe("ready");
  });

  it("bar chart: still gated during the reveal with full motion (control)", () => {
    const phases: string[] = [];
    render(
      wrap(
        "full",
        <BarChart data={BARS} onPhaseChange={(p) => phases.push(p)}>
          <Bar dataKey="v" />
        </BarChart>,
      ),
    );
    expect(phases.at(-1)).toBe("revealing");
    act(() => {
      vi.advanceTimersByTime(2_000);
    });
    expect(phases.at(-1)).toBe("ready");
  });

  it("time-series chart: ready without waiting for the reveal timer", () => {
    const phases: string[] = [];
    render(
      wrap(
        "reduced",
        <LineChart data={POINTS} xDataKey="date" onPhaseChange={(p) => phases.push(p)}>
          <Line dataKey="v" />
        </LineChart>,
      ),
    );
    expect(phases.at(-1)).toBe("ready");
  });
});
