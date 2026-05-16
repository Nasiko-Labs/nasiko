import { Activity, CircleAlert, GitBranch, Terminal, Zap } from "lucide-react";
import type { LogStats } from "../types";

type TopBarProps = {
  commit: string;
  stats: LogStats;
};

export function TopBar({ commit, stats }: TopBarProps) {
  return (
    <header className="topbar" aria-labelledby="logs-title">
      <div className="product-lockup">
        <div className="brand-mark" aria-hidden="true">
          <Terminal size={18} strokeWidth={2.4} />
        </div>
        <div>
          <p className="eyebrow">Nasiko observability</p>
          <h1 id="logs-title">Platform Logs</h1>
        </div>
      </div>

      <div className="signals" aria-label="Log summary">
        <Signal icon={<CircleAlert size={14} />} label="Errors" value={String(stats.errors)} tone="error" />
        <Signal icon={<Zap size={14} />} label="P95" value={`${stats.p95LatencyMs}ms`} tone="warn" />
        <Signal icon={<Activity size={14} />} label="Events" value={String(stats.total)} tone="info" />
        <Signal icon={<GitBranch size={14} />} label="Commit" value={commit} tone="neutral" />
      </div>
    </header>
  );
}

type SignalProps = {
  icon: React.ReactNode;
  label: string;
  tone: "error" | "warn" | "info" | "neutral";
  value: string;
};

function Signal({ icon, label, tone, value }: SignalProps) {
  return (
    <div className={`signal ${tone}`}>
      <span aria-hidden="true">{icon}</span>
      <small>{label}</small>
      <strong>{value}</strong>
    </div>
  );
}
