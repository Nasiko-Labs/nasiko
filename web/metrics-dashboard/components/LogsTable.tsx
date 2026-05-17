"use client";

import { useState } from "react";
import { formatDateTime, formatLatencyMs, formatShortId } from "@/lib/format";
import type { LogLevel, SyntheticLogRow } from "@/lib/types";

type Props = {
  rows: SyntheticLogRow[];
  loading?: boolean;
};

const LEVEL_CLASS: Record<LogLevel, string> = {
  INFO: "log-badge--info",
  WARNING: "log-badge--warn",
  ERROR: "log-badge--error",
};

function LogRow({ row }: { row: SyntheticLogRow }) {
  const [open, setOpen] = useState(false);
  const hasDetail =
    row.detail != null && typeof row.detail === "object" && Object.keys(row.detail).length > 0;
  return (
    <>
      <tr
        className={hasDetail ? "log-row log-row--clickable" : "log-row"}
        onClick={hasDetail ? () => setOpen((v) => !v) : undefined}
        aria-expanded={hasDetail ? open : undefined}
      >
        <td className="log-row__time">{formatDateTime(row.ts)}</td>
        <td>
          <span className={`log-badge ${LEVEL_CLASS[row.level]}`}>
            {row.level}
          </span>
        </td>
        <td className="log-row__service">{row.service}</td>
        <td className="log-row__message">{row.message}</td>
        <td className="log-row__meta">
          {row.session_id ? (
            <code title={row.session_id}>{formatShortId(row.session_id)}</code>
          ) : (
            "—"
          )}
        </td>
        <td className="log-row__meta">
          {row.latency_ms != null ? formatLatencyMs(row.latency_ms) : "—"}
        </td>
      </tr>
      {hasDetail && open && (
        <tr className="log-row__detail">
          <td colSpan={6}>
            <pre>{JSON.stringify(row.detail, null, 2)}</pre>
          </td>
        </tr>
      )}
    </>
  );
}

export function LogsTable({ rows, loading }: Props) {
  return (
    <section
      className="panel logs-panel"
      aria-busy={loading || undefined}
      aria-label="Recent platform logs"
    >
      <div className="logs-table-wrap">
        <table className="logs-table">
          <thead>
            <tr>
              <th scope="col">Time</th>
              <th scope="col">Level</th>
              <th scope="col">Service</th>
              <th scope="col">Message</th>
              <th scope="col">Session</th>
              <th scope="col">Latency</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <LogRow key={row.id} row={row} />
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
