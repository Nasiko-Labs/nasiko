import {
  Bar,
  BarChart,
  CartesianGrid,
  Legend,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import type { HourlyBucket } from "../types";

interface MetricsChartsProps {
  hourly: HourlyBucket[];
}

function formatHourLabel(isoHour: string): string {
  const date = new Date(isoHour);
  return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

export default function MetricsCharts({ hourly }: MetricsChartsProps) {
  const chartData = hourly.map((bucket) => ({
    ...bucket,
    label: formatHourLabel(bucket.hour),
    success: Math.max(bucket.requests - bucket.errors, 0),
  }));

  return (
    <div className="charts-grid">
      <div className="panel chart-card">
        <h3>Requests (last 24h)</h3>
        <ResponsiveContainer width="100%" height={260}>
          <BarChart data={chartData}>
            <CartesianGrid strokeDasharray="3 3" stroke="#2a3647" />
            <XAxis dataKey="label" tick={{ fill: "#8fa3bd", fontSize: 11 }} />
            <YAxis tick={{ fill: "#8fa3bd", fontSize: 11 }} allowDecimals={false} />
            <Tooltip
              contentStyle={{
                background: "#121820",
                border: "1px solid #2a3647",
                borderRadius: 8,
              }}
            />
            <Legend />
            <Bar dataKey="success" stackId="a" fill="#3dd68c" name="Success" />
            <Bar dataKey="errors" stackId="a" fill="#ff6b6b" name="Errors" />
          </BarChart>
        </ResponsiveContainer>
      </div>

      <div className="panel chart-card">
        <h3>Avg response time (ms)</h3>
        <ResponsiveContainer width="100%" height={260}>
          <LineChart data={chartData}>
            <CartesianGrid strokeDasharray="3 3" stroke="#2a3647" />
            <XAxis dataKey="label" tick={{ fill: "#8fa3bd", fontSize: 11 }} />
            <YAxis tick={{ fill: "#8fa3bd", fontSize: 11 }} />
            <Tooltip
              contentStyle={{
                background: "#121820",
                border: "1px solid #2a3647",
                borderRadius: 8,
              }}
            />
            <Line
              type="monotone"
              dataKey="avg_latency_ms"
              stroke="#4f8cff"
              strokeWidth={2}
              dot={false}
              name="Avg latency (ms)"
            />
          </LineChart>
        </ResponsiveContainer>
      </div>
    </div>
  );
}
