"use client";

import { Bar, BarChart, ChartTooltip } from "@nasiko/ui/charts-kit";

/**
 * The vendored chart kit under the motion host (test:motion `chart-kit.motion.ts`): a kit bar chart inside the
 * harness surface, so `?profile=none`, `?preference=reduced`, `?ceiling=off` and the OS setting reach it the way
 * they reach a product surface. The bars use a fixed fill so the test finds them; a tooltip is mounted so hovering
 * exercises the kit's tooltip motion under the app's strict `LazyMotion`.
 */
const KIT_CHART_FILL = "rgb(37, 99, 235)";
const DATA = [
  { name: "Mon", v: 12 },
  { name: "Tue", v: 30 },
  { name: "Wed", v: 21 },
  { name: "Thu", v: 44 },
];

export default function ChartKitFixture() {
  return (
    <figure data-testid="kit-chart" aria-label="Fixture bars" className="h-48 w-[480px]">
      <BarChart
        data={DATA}
        aspectRatio="auto"
        className="h-full"
        margin={{ top: 8, right: 8, bottom: 8, left: 8 }}
      >
        <Bar dataKey="v" fill={KIT_CHART_FILL} />
        <ChartTooltip
          showDatePill={false}
          rows={(p) => [{ label: "v", value: Number(p.v), color: KIT_CHART_FILL }]}
        />
      </BarChart>
    </figure>
  );
}
