import type { RunTotals } from "../../shared/types.js";
import { severityForScoreClient } from "../score.js";

function Card({
  label,
  value,
  accent,
}: {
  label: string;
  value: string | number;
  accent?: string;
}) {
  return (
    <div className="rounded-lg border border-edge bg-panel p-4">
      <div className="text-[11px] uppercase tracking-wider text-slate-500">
        {label}
      </div>
      <div className={`text-3xl font-bold mt-1 ${accent ?? "text-white"}`}>
        {value}
      </div>
    </div>
  );
}

export function Metrics({
  risk,
  totals,
  threats,
}: {
  risk: number;
  totals: RunTotals;
  threats: number;
}) {
  const sev = severityForScoreClient(risk);
  const riskColor = {
    low: "text-low",
    medium: "text-medium",
    high: "text-high",
    critical: "text-critical",
  }[sev];

  return (
    <div className="grid grid-cols-2 md:grid-cols-5 gap-4">
      <Card label="Risk Score" value={`${risk}/100`} accent={riskColor} />
      <Card label="Tool Calls" value={totals.toolCalls} />
      <Card label="Executed" value={totals.executed} accent="text-low" />
      <Card label="Blocked" value={totals.blocked} accent="text-critical" />
      <Card label="Threats" value={threats} accent="text-high" />
    </div>
  );
}
