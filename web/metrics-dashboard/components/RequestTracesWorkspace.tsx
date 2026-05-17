"use client";

import { useEffect, useMemo, useState } from "react";
import { TraceGraphPanel } from "@/components/TraceGraphPanel";
import {
  filterSessionRows,
  type SessionStatusFilter,
} from "@/lib/session-filters";
import { formatDateTime, formatLatencyMs, formatShortId } from "@/lib/format";
import { formatRangeLabel } from "@/lib/time-range";
import type { MetricsSessionRow, TraceDetailResponse } from "@/lib/types";
import type { MetricsTimeRange } from "@/lib/time-range";

const PAGE_SIZE = 10;

type Props = {
  sessions: MetricsSessionRow[];
  globalRange: MetricsTimeRange;
  loading?: boolean;
  selectedSessionId: string | null;
  traceLoadingSessionId: string | null;
  trace: TraceDetailResponse | null;
  traceLoading: boolean;
  traceError: string | null;
  onSelectSession: (sessionId: string) => void;
};

function statusLabel(status: MetricsSessionRow["status"]): string {
  if (status === "ok") return "Success";
  if (status === "error") return "Failed";
  return "No data";
}

export function RequestTracesWorkspace({
  sessions,
  globalRange,
  loading,
  selectedSessionId,
  traceLoadingSessionId,
  trace,
  traceLoading,
  traceError,
  onSelectSession,
}: Props) {
  const [page, setPage] = useState(0);
  const [fullscreen, setFullscreen] = useState(false);
  const [statusFilter, setStatusFilter] = useState<SessionStatusFilter>("all");

  const filteredSessions = useMemo(
    () => filterSessionRows(sessions, { status: statusFilter }),
    [sessions, statusFilter],
  );

  const pageCount = Math.max(1, Math.ceil(filteredSessions.length / PAGE_SIZE));

  useEffect(() => {
    setPage(0);
  }, [filteredSessions.length, statusFilter, globalRange.startMs, globalRange.endMs]);

  useEffect(() => {
    if (page > pageCount - 1) {
      setPage(Math.max(0, pageCount - 1));
    }
  }, [page, pageCount]);

  useEffect(() => {
    if (!fullscreen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setFullscreen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [fullscreen]);

  const pageSessions = useMemo(() => {
    const start = page * PAGE_SIZE;
    return filteredSessions.slice(start, start + PAGE_SIZE);
  }, [page, filteredSessions]);

  const canvasBlock = (
    <TraceGraphPanel
      trace={trace}
      loading={traceLoading}
      error={traceError}
      sessionId={selectedSessionId}
      fullscreen={fullscreen}
      onToggleFullscreen={() => setFullscreen((v) => !v)}
      compact
    />
  );

  if (fullscreen) {
    return (
      <section className="panel trace-workspace trace-workspace--fullscreen-mode">
        <div className="trace-workspace__fullscreen-shell">
          {canvasBlock}
          <button
            type="button"
            className="btn trace-workspace__close-fs"
            onClick={() => setFullscreen(false)}
          >
            Close
          </button>
        </div>
      </section>
    );
  }

  if (loading && sessions.length === 0) {
    return (
      <section className="panel trace-workspace" aria-busy="true">
        <h2 className="panel__title">Request traces</h2>
        <p className="panel__hint">Loading requests…</p>
      </section>
    );
  }

  if (sessions.length === 0) {
    return (
      <section className="panel trace-workspace">
        <h2 className="panel__title">Request traces</h2>
        <p className="panel__hint">
          No requests in the selected time range ({formatRangeLabel(globalRange)}).
          Widen the range above or send traffic to this agent.
        </p>
      </section>
    );
  }

  return (
    <section className="panel trace-workspace" aria-labelledby="traces-heading">
      <header className="trace-workspace__header">
        <div>
          <h2 id="traces-heading" className="panel__title">
            Request traces
          </h2>
          <p className="panel__hint">
            {filteredSessions.length} of {sessions.length} in{" "}
            {formatRangeLabel(globalRange)}
            {filteredSessions.length > 0
              ? ` · page ${page + 1} of ${pageCount}`
              : ""}
          </p>
        </div>
      </header>

      <div className="trace-workspace__split">
        <div className="trace-workspace__list">
          <div className="trace-workspace__toolbar" role="toolbar" aria-label="Filter requests">
            <div className="trace-filter-chips" role="group" aria-label="Status">
              {(
                [
                  ["all", "All"],
                  ["ok", "Success"],
                  ["error", "Failed"],
                ] as const
              ).map(([value, label]) => (
                <button
                  key={value}
                  type="button"
                  className={`trace-filter-chip${statusFilter === value ? " trace-filter-chip--active" : ""}`}
                  aria-pressed={statusFilter === value}
                  disabled={loading}
                  onClick={() => setStatusFilter(value)}
                >
                  {label}
                </button>
              ))}
            </div>
            <p className="trace-workspace__toolbar-hint">
              Time range uses the filter above the chart. Click a row to view its path →
            </p>
          </div>

          {filteredSessions.length === 0 ? (
            <p className="panel__hint trace-workspace__empty">
              No {statusFilter === "all" ? "" : statusFilter === "ok" ? "successful " : "failed "}
              requests in this range.
            </p>
          ) : (
            <>
              <div className="trace-workspace__table-scroll">
                <table className="data-table data-table--requests">
                  <thead>
                    <tr>
                      <th scope="col">Time</th>
                      <th scope="col">Preview</th>
                      <th scope="col">Latency</th>
                      <th scope="col">Status</th>
                    </tr>
                  </thead>
                  <tbody>
                    {pageSessions.map((row) => {
                      const isSelected = selectedSessionId === row.session_id;
                      const isRowLoading = traceLoadingSessionId === row.session_id;
                      const clickable = row.num_traces > 0;
                      return (
                        <tr
                          key={row.session_id}
                          data-selected={isSelected || undefined}
                          data-status={row.status}
                          className={
                            clickable ? "data-table__row--clickable" : "data-table__row--muted"
                          }
                          onClick={() => {
                            if (clickable) onSelectSession(row.session_id);
                          }}
                        >
                          <td>
                            <time dateTime={row.start_time ?? undefined}>
                              {formatDateTime(row.start_time)}
                            </time>
                            <span className="table-subtle" title={row.session_id}>
                              {" "}
                              · {formatShortId(row.session_id)}
                            </span>
                          </td>
                          <td className="data-table__preview" title={row.preview ?? undefined}>
                            {row.preview ?? "Conversation with this agent"}
                          </td>
                          <td>{formatLatencyMs(row.latency_ms_p50)}</td>
                          <td>
                            <span className={`badge badge--${row.status}`}>
                              {isRowLoading ? "Loading…" : statusLabel(row.status)}
                            </span>
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>

              <div className="pagination" role="navigation" aria-label="Request pages">
                <button
                  type="button"
                  className="btn btn--sm"
                  disabled={page <= 0}
                  onClick={() => setPage((p) => Math.max(0, p - 1))}
                >
                  Previous
                </button>
                <span className="pagination__label">
                  {page * PAGE_SIZE + 1}–
                  {Math.min((page + 1) * PAGE_SIZE, filteredSessions.length)} of{" "}
                  {filteredSessions.length}
                </span>
                <button
                  type="button"
                  className="btn btn--sm"
                  disabled={page >= pageCount - 1}
                  onClick={() => setPage((p) => Math.min(pageCount - 1, p + 1))}
                >
                  Next
                </button>
              </div>
            </>
          )}
        </div>

        <div className="trace-workspace__canvas">{canvasBlock}</div>
      </div>
    </section>
  );
}
