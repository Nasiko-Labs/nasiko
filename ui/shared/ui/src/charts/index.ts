// Lazy chart entry (ADR-0003). Import from "@nasiko/ui/charts" only behind a
// dynamic import boundary (React.lazy); check:bundles asserts
// the neutral route never carries recharts.
export {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  ChartLegend,
  ChartLegendContent,
  isSafeChartColor,
  chartStyleVars,
  type ChartConfig,
} from "./chart";
export {
  TimeSeriesChart,
  CategoryBarChart,
  ComboChart,
  MAX_SERIES,
  DEFAULT_MAX_POINTS,
  type ChartShape,
  type SeriesChartProps,
  type SeriesPoint,
  type SeriesSpec,
} from "./series";
export { Gauge, type GaugeProps } from "./gauge";
export { Heatmap, HEATMAP_MAX_CELLS, type HeatmapProps } from "./heatmap";
export { DonutChart, DONUT_MAX_SLICES, type DonutChartProps, type DonutSlice } from "./donut";
