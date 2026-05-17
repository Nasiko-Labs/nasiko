"use client";

import {
  formatCostUsd,
  formatCount,
  formatLatencyMs,
  formatPercent,
} from "@/lib/format";
import type { FleetAgentRow } from "@/lib/fleet-rollup";

type Props = {
  rows: FleetAgentRow[];
  loading?: boolean;
  onSelectAgent: (agentId: string) => void;
};

export function AgentsOverviewTable({ rows, loading, onSelectAgent }: Props) {
  if (loading) {
    return (
      <section className="panel">
        <h2 className="panel__title">By agent</h2>
        <p className="panel__hint">Loading…</p>
      </section>
    );
  }

  if (rows.length === 0) {
    return (
      <section className="panel">
        <h2 className="panel__title">By agent</h2>
        <p className="panel__hint">Deploy agents in the main app to see them here.</p>
      </section>
    );
  }

  return (
    <section className="panel" aria-labelledby="agents-overview-heading">
      <h2 id="agents-overview-heading" className="panel__title">
        By agent
      </h2>
      <p className="panel__hint">
        Compare each agent side by side. Open one for charts and recent requests.
      </p>
      <div className="table-wrap">
        <table className="data-table data-table--overview">
          <thead>
            <tr>
              <th scope="col">Agent</th>
              <th scope="col">Response time</th>
              <th scope="col">Successful</th>
              <th scope="col">Failed</th>
              <th scope="col">Availability</th>
              <th scope="col">Est. cost</th>
              <th scope="col">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => {
              const m = row.metrics;
              const noData = m.no_data === true;
              return (
                <tr key={row.agentId} data-muted={noData || undefined}>
                  <td>
                    <strong>{row.name}</strong>
                    {noData && (
                      <span className="table-subtle"> · No traffic yet</span>
                    )}
                  </td>
                  <td>{noData ? "—" : formatLatencyMs(m.avg_latency_ms)}</td>
                  <td>{noData ? "—" : formatCount(m.success)}</td>
                  <td>{noData ? "—" : formatCount(m.error)}</td>
                  <td>{noData ? "—" : formatPercent(m.uptime_pct)}</td>
                  <td>{noData ? "—" : formatCostUsd(m.source?.cost_usd)}</td>
                  <td>
                    <button
                      type="button"
                      className="btn btn--sm"
                      onClick={() => onSelectAgent(row.agentId)}
                    >
                      View details
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </section>
  );
}
