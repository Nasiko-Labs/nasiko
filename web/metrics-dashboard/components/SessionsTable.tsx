"use client";

import { formatDateTime, formatLatencyMs, formatShortId } from "@/lib/format";
import type { MetricsSessionRow } from "@/lib/types";

type Props = {
  sessions: MetricsSessionRow[];
  loading?: boolean;
  selectedSessionId: string | null;
  traceLoadingSessionId: string | null;
  onViewTrace: (sessionId: string) => void;
};

export function SessionsTable({
  sessions,
  loading,
  selectedSessionId,
  traceLoadingSessionId,
  onViewTrace,
}: Props) {
  if (loading) {
    return (
      <section className="panel" aria-busy="true">
        <h2 className="panel__title">Recent sessions</h2>
        <p className="panel__hint">Loading sessions…</p>
      </section>
    );
  }

  if (sessions.length === 0) {
    return (
      <section className="panel">
        <h2 className="panel__title">Recent sessions</h2>
        <p className="panel__hint">
          No Phoenix sessions in the last 24 hours. Chat with the agent to create traces.
        </p>
      </section>
    );
  }

  return (
    <section className="panel" aria-labelledby="sessions-heading">
      <h2 id="sessions-heading" className="panel__title">
        Recent sessions
      </h2>
      <p className="panel__hint">
        Select a session to load its request trace as a span graph below.
      </p>
      <div className="table-wrap">
        <table className="data-table">
          <thead>
            <tr>
              <th scope="col">Time</th>
              <th scope="col">Session</th>
              <th scope="col">Traces</th>
              <th scope="col">Latency</th>
              <th scope="col">Status</th>
              <th scope="col">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {sessions.slice(0, 20).map((row) => {
              const isSelected = selectedSessionId === row.session_id;
              const isLoading = traceLoadingSessionId === row.session_id;
              return (
                <tr
                  key={row.session_id}
                  data-selected={isSelected || undefined}
                  data-status={row.status}
                >
                  <td>{formatDateTime(row.start_time)}</td>
                  <td>
                    <code title={row.session_id}>{formatShortId(row.session_id)}</code>
                  </td>
                  <td>{row.num_traces}</td>
                  <td>{formatLatencyMs(row.latency_ms_p50)}</td>
                  <td>
                    <span className={`badge badge--${row.status}`}>{row.status}</span>
                  </td>
                  <td>
                    <button
                      type="button"
                      className="btn btn--sm"
                      disabled={row.num_traces <= 0 || isLoading}
                      onClick={() => onViewTrace(row.session_id)}
                      aria-pressed={isSelected}
                    >
                      {isLoading ? "Loading…" : "View trace"}
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
