import type { Meta, StoryObj } from "@storybook/react-vite";
import { Gauge } from "./gauge";
import { Heatmap } from "./heatmap";
import { CategoryBarChart, TimeSeriesChart } from "./series";

/** Synthetic, seeded data: the same numbers every run (ADR-0003 side-by-side review). */
const days = Array.from({ length: 30 }, (_, i) => {
  const d = new Date(Date.UTC(2026, 8, 1 + i));
  const seed = (i * 9301 + 49297) % 233280;
  return {
    day: d.toISOString().slice(5, 10),
    spend: Math.round(120 + (seed / 233280) * 80),
    tokens: Math.round(40 + (seed / 233280) * 60),
    errors: i % 7 === 3 ? null : Math.round((seed / 233280) * 5),
  };
});
const usd = (v: number) => `$${v.toFixed(0)}`;

const meta = { title: "Charts/Overview" } satisfies Meta;
export default meta;

export const TimeSeries: StoryObj = {
  render: () => (
    <TimeSeriesChart
      description="Daily spend and token volume, last 30 days"
      data={days}
      xKey="day"
      series={[
        { key: "spend", label: "Spend (USD)" },
        { key: "tokens", label: "Tokens (k)" },
      ]}
      formatY={usd}
    />
  ),
};

export const StackedArea: StoryObj = {
  render: () => (
    <TimeSeriesChart
      stacked
      description="Stacked spend and tokens"
      data={days}
      xKey="day"
      series={[
        { key: "spend", label: "Spend" },
        { key: "tokens", label: "Tokens" },
      ]}
    />
  ),
};

export const MissingPeriods: StoryObj = {
  name: "Line with missing periods (no tween across gaps)",
  render: () => (
    <TimeSeriesChart
      description="Errors per day with gaps"
      data={days}
      xKey="day"
      series={[{ key: "errors", label: "Errors", slot: 2 }]}
    />
  ),
};

export const CategoryBars: StoryObj = {
  render: () => (
    <CategoryBarChart
      description="Spend by agent, top 6 + other"
      data={[
        { agent: "alpha", spend: 410 },
        { agent: "beta", spend: 320 },
        { agent: "gamma", spend: 210 },
        { agent: "delta", spend: 120 },
        { agent: "epsilon", spend: 90 },
        { agent: "zeta", spend: 40 },
        { agent: "other", spend: 35 },
      ]}
      xKey="agent"
      series={[{ key: "spend", label: "Spend" }]}
      formatY={usd}
    />
  ),
};

export const GaugeStory: StoryObj = {
  name: "Gauge",
  render: () => (
    <div className="flex flex-wrap gap-6">
      <Gauge
        label="Budget used"
        value={62}
        max={100}
        formatValue={(v) => `${v}%`}
        thresholds={[0.75, 0.9]}
      />
      <Gauge
        label="Budget used"
        value={93}
        max={100}
        formatValue={(v) => `${v}%`}
        thresholds={[0.75, 0.9]}
      />
      <Gauge label="Budget used" value={null} max={100} formatValue={(v) => `${v}%`} />
    </div>
  ),
};

export const HeatmapStory: StoryObj = {
  name: "Heatmap (keyboard: arrows, Enter)",
  render: () => {
    const rows = ["alpha", "beta", "gamma", "delta"];
    const cols = Array.from({ length: 24 }, (_, h) => `${h}h`);
    const values = rows.map((_, r) =>
      cols.map((_, c) => (c % 11 === r ? null : ((r + 1) * (c + 3) * 7) % 50)),
    );
    return (
      <Heatmap label="Requests by agent and hour" rows={rows} columns={cols} values={values} />
    );
  },
};
