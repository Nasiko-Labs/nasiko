import { PlatformLogEntry } from "../api/client";

interface LogsTableProps {
  logs: PlatformLogEntry[];
  loading: boolean;
}

function formatTimestamp(iso: string): string {
  try {
    return new Date(iso).toLocaleString(undefined, {
      year: "numeric",
      month: "short",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
  } catch {
    return iso;
  }
}

export function LogsTable({ logs, loading }: LogsTableProps) {
  if (loading && logs.length === 0) {
    return <p className="empty">Loading platform logs…</p>;
  }

  if (!loading && logs.length === 0) {
    return <p className="empty">No logs match this filter.</p>;
  }

  return (
    <div className="table-wrap">
      <table className="logs-table">
        <thead>
          <tr>
            <th>Timestamp</th>
            <th>Level</th>
            <th>Service</th>
            <th>Message</th>
          </tr>
        </thead>
        <tbody>
          {logs.map((log) => (
            <tr key={log.id ?? `${log.timestamp}-${log.message}`}>
              <td className="mono ts">{formatTimestamp(log.timestamp)}</td>
              <td>
                <span className={`badge level-${log.level.toLowerCase()}`}>
                  {log.level}
                </span>
              </td>
              <td className="mono service">{log.service}</td>
              <td className="message">{log.message}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
