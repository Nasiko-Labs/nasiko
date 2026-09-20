import type { SecurityEvent } from "../../shared/types.js";
import { outcomeLabel } from "../ui.js";

export function WhyPanel({ event }: { event: SecurityEvent }) {
  const t = event.telemetry as Record<string, unknown>;
  const evidence: string[] = [];
  if (t.action) evidence.push(`Operation: ${t.action}`);
  if (t.simulated_rows) evidence.push(`${t.simulated_rows} simulated rows`);
  if (t.table) evidence.push(`Target table: ${t.table}`);
  if (t.requested) evidence.push(`Requested secret: ${t.requested}`);
  if (t.path) evidence.push(`Path: ${t.path}`);
  if (t.to) evidence.push(`Recipient: ${t.to}`);
  if (t.external) evidence.push("External destination");
  if (t.command) evidence.push(`Command: ${t.command}`);
  evidence.push(`Decision by: ${event.decision.policyLabel}`);

  const decisionColor =
    event.decision.access === "denied"
      ? "text-critical"
      : event.decision.access === "ask"
        ? "text-medium"
        : "text-low";

  return (
    <div className="rounded-lg border border-accent/40 bg-accent/5">
      <div className="px-4 py-3 border-b border-edge text-xs uppercase tracking-wider text-accent">
        Why was this {outcomeLabel(event.outcome).toLowerCase()}?
      </div>
      <div className="p-4 space-y-3 text-sm">
        <Row label="Tool" value={event.tool} mono />
        <Row
          label="Decision"
          value={event.decision.access.toUpperCase()}
          className={decisionColor}
        />
        <div>
          <div className="text-[10px] uppercase text-slate-500 mb-1">
            Policy
          </div>
          <p className="text-slate-300 leading-relaxed">
            {event.decision.reason}
          </p>
        </div>
        <Row
          label="Risk"
          value={event.severity.toUpperCase()}
          className="text-high"
        />
        <div>
          <div className="text-[10px] uppercase text-slate-500 mb-1">
            Evidence
          </div>
          <ul className="text-slate-400 text-xs space-y-0.5">
            {evidence.map((e, i) => (
              <li key={i}>• {e}</li>
            ))}
          </ul>
        </div>
      </div>
    </div>
  );
}

function Row({
  label,
  value,
  className,
  mono,
}: {
  label: string;
  value: string;
  className?: string;
  mono?: boolean;
}) {
  return (
    <div className="flex items-center justify-between">
      <span className="text-[10px] uppercase text-slate-500">{label}</span>
      <span
        className={`font-semibold ${mono ? "font-mono" : ""} ${className ?? "text-white"}`}
      >
        {value}
      </span>
    </div>
  );
}
