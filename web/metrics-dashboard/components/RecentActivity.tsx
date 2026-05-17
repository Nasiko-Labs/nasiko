"use client";

import { formatDateTime, formatLatencyMs } from "@/lib/format";
import type { MetricsSessionRow } from "@/lib/types";

type Props = {
  sessions: MetricsSessionRow[];
  loading?: boolean;
  onInspect?: (sessionId: string) => void;
  inspectLoadingId?: string | null;
  showTechnicalActions?: boolean;
};

function statusLabel(status: MetricsSessionRow["status"]): string {
  if (status === "ok") return "Succeeded";
  if (status === "error") return "Failed";
  return "No data";
}

export function RecentActivity({
  sessions,
  loading,
  onInspect,
  inspectLoadingId,
  showTechnicalActions = false,
}: Props) {
  if (loading) {
    return (
      <section className="panel">
        <h2 className="panel__title">Recent customer requests</h2>
        <p className="panel__hint">Loading…</p>
      </section>
    );
  }

  if (sessions.length === 0) {
    return (
      <section className="panel">
        <h2 className="panel__title">Recent customer requests</h2>
        <p className="panel__hint">
          When someone uses this agent, each conversation will appear here with whether
          it worked and how long it took.
        </p>
      </section>
    );
  }

  return (
    <section className="panel" aria-labelledby="activity-heading">
      <h2 id="activity-heading" className="panel__title">
        Recent customer requests
      </h2>
      <p className="panel__hint">
        The latest conversations in the last 24 hours — newest first.
      </p>
      <ul className="activity-list">
        {sessions.slice(0, 10).map((row) => {
          const isLoading = inspectLoadingId === row.session_id;
          return (
            <li key={row.session_id} className="activity-item" data-status={row.status}>
              <div className="activity-item__main">
                <p className="activity-item__when">{formatDateTime(row.start_time)}</p>
                <p className="activity-item__preview">
                  {row.preview ?? "Conversation with this agent"}
                </p>
              </div>
              <aside className="activity-item__stats">
                <span className={`badge badge--${row.status}`}>
                  {statusLabel(row.status)}
                </span>
                <span className="activity-item__time">
                  {formatLatencyMs(row.latency_ms_p50)}
                </span>
                {showTechnicalActions && onInspect && (
                  <button
                    type="button"
                    className="btn btn--sm btn--ghost"
                    disabled={row.num_traces <= 0 || isLoading}
                    onClick={() => onInspect(row.session_id)}
                  >
                    {isLoading ? "Loading…" : "Engineering details"}
                  </button>
                )}
              </aside>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
