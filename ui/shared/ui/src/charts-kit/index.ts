// The vendored visx chart kit (README.md, NOTICE): a lazy entry of @nasiko/ui, separate from
// "@nasiko/ui/charts" (Recharts). Import it only behind a dynamic import boundary. This is the
// surface TokenOps composes its charts from.
export { ComposedChart, type ComposedChartProps } from "./composed-chart";
export { BarChart, type BarChartProps, type BarOrientation } from "./bar-chart";
export { LineChart, type LineChartProps } from "./line-chart";
export { SeriesBar, type SeriesBarProps } from "./series-bar";
export { Bar, type BarProps, type BarAnimationType, type BarLineCap } from "./bar";
export { Line, type LineProps } from "./line";
export { Area, type AreaProps } from "./area";
export { Grid, type GridProps } from "./grid";
export { XAxis, type XAxisProps } from "./x-axis";
export { BarXAxis, type BarXAxisProps } from "./bar-x-axis";
export { YAxis, type YAxisProps } from "./y-axis";
export { ReferenceLine } from "./reference-line";
export { HatchPattern } from "./hatch-pattern";
export { ChartTooltip, type ChartTooltipProps } from "./tooltip/chart-tooltip";
export { ChartHover } from "./chart-hover";
export {
  ChartDateFormat,
  shortDateFmt,
  weekdayDateFmt,
  hmsTimeFmt,
  intFmt,
} from "./chart-formatters";
export type { TooltipData } from "./chart-context";
export { seriesAt, OTHER_SERIES, type Series } from "./lib/series";
export * from "./lib/format";
// TokenOps' cost × p95 scatter on raw visx (ported from the lab's features/tokenops/components/CostScatter.tsx).
export { CostScatter, type CostScatterProps, type ScatterPoint } from "./cost-scatter";
// TokenOps' hero spend curve (legacy ui/ on development, 3aac84f66: 8bf09bf29) draws with d3-shape directly; the
// UI package owns that dependency, so features take its helpers from here.
export { area, curveMonotoneX, line } from "d3-shape";
