// The OS reduced-motion half of `motion-policy.test.tsx`: the media query reports `reduce` before motion reads
// it (motion caches it per module registry, which vitest isolates per file).
import { render } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeAll, describe, expect, it, vi } from "vitest";
import { MotionProvider } from "../motion/provider";
import { Bar } from "./bar";
import { BarChart } from "./bar-chart";

vi.mock("@visx/responsive", () => ({
  ParentSize: ({ children }: { children: (s: { width: number; height: number }) => ReactNode }) =>
    children({ width: 400, height: 200 }),
}));

beforeAll(() => {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: (query: string) => ({
      // motion asks "(prefers-reduced-motion)", the host "(prefers-reduced-motion: reduce)".
      matches: /^\(prefers-reduced-motion(: reduce)?\)$/.test(query),
      media: query,
      onchange: null,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
      addListener: () => undefined,
      removeListener: () => undefined,
      dispatchEvent: () => false,
    }),
  });
});

const FILL = "rgb(1, 2, 3)";
const DATA = [
  { name: "a", v: 10 },
  { name: "b", v: 40 },
];

function Chart() {
  return (
    <BarChart data={DATA} margin={{ top: 0, right: 0, bottom: 0, left: 0 }}>
      <Bar dataKey="v" fill={FILL} />
    </BarChart>
  );
}

function barHeights(container: HTMLElement): number[] {
  return [...container.querySelectorAll(`rect[fill="${FILL}"]`)].map((r) =>
    Number.parseFloat(r.getAttribute("height") ?? "NaN"),
  );
}

describe("chart kit under OS reduced motion", () => {
  it("draws bars at rest outside any provider (the lab's own rule)", () => {
    const { container } = render(<Chart />);
    const h = barHeights(container);
    expect(h).toHaveLength(2);
    expect(h[1]).toBeGreaterThan(h[0] ?? 0);
    expect(h[0]).toBeGreaterThan(0);
  });

  it("draws bars at rest inside an app-scope provider reading the OS", () => {
    const { container } = render(
      <MotionProvider scope="app" profile="expressive">
        <Chart />
      </MotionProvider>,
    );
    const h = barHeights(container);
    expect(h).toHaveLength(2);
    for (const x of h) expect(x).toBeGreaterThan(0);
  });
});
