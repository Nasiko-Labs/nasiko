import { Copy, FileJson2, Info, TerminalSquare, TriangleAlert, XCircle } from "lucide-react";
import type { LogLevel, PlatformLog } from "../types";
import { createEventJson, createKubectlCommand, formatClock, formatFullTimestamp } from "../utils/logs";

type LogStreamProps = {
  copiedKey: string | null;
  expandedLogId?: string;
  logs: PlatformLog[];
  onCopy: (key: string, value: string) => void;
  onToggle: (logId: string) => void;
};

const levelIcons: Record<LogLevel, typeof Info> = {
  INFO: Info,
  WARNING: TriangleAlert,
  ERROR: XCircle
};

export function LogStream({ copiedKey, expandedLogId, logs, onCopy, onToggle }: LogStreamProps) {
  return (
    <section className="logs-card" aria-label="Recent platform logs">
      <div className="logs-header">
        <div>
          <h2>Recent platform logs</h2>
          <p>{logs.length} entries shown</p>
        </div>
        <code>tail -f platform.log</code>
      </div>

      <div className="log-table" role="table" aria-label="Platform log table">
        <div className="log-head" role="row">
          <span role="columnheader">Time</span>
          <span role="columnheader">Level</span>
          <span role="columnheader">Service</span>
          <span role="columnheader">Message</span>
          <span role="columnheader">Latency</span>
          <span role="columnheader">Trace</span>
        </div>

        {logs.map((log) => {
          const isExpanded = expandedLogId === log.id;
          const Icon = levelIcons[log.level];
          const eventJson = createEventJson(log);
          const kubectlCommand = createKubectlCommand(log);

          return (
            <article className={`log-entry ${log.level.toLowerCase()}`} key={log.id}>
              <button
                aria-expanded={isExpanded}
                className="log-row"
                onClick={() => onToggle(log.id)}
                type="button"
              >
                <span className="time-cell" data-label="Time" role="cell">
                  <time dateTime={log.timestamp} title={formatFullTimestamp(log.timestamp)}>
                    {formatClock(log.timestamp)}
                  </time>
                  <small>{log.requestId}</small>
                </span>

                <span data-label="Level" role="cell">
                  <span className={`level-pill ${log.level.toLowerCase()}`}>
                    <Icon size={13} aria-hidden="true" />
                    {log.level}
                  </span>
                </span>

                <span className="service-cell" data-label="Service" role="cell">
                  {log.service}
                </span>

                <span className="message-cell" data-label="Message" role="cell">
                  <strong>{log.message}</strong>
                  <small>{log.route}</small>
                </span>

                <span className={log.latencyMs >= 1000 ? "latency-cell slow" : "latency-cell"} data-label="Latency" role="cell">
                  {log.latencyMs}ms
                </span>

                <span className="trace-cell" data-label="Trace" role="cell">
                  <code>{log.traceId}</code>
                  <span>open</span>
                </span>
              </button>

              {isExpanded ? (
                <div className="log-detail">
                  <div className="detail-grid">
                    <Detail label="Pod" value={log.pod} />
                    <Detail label="Source" value={log.source} />
                    <Detail label="Commit" value={log.commit} />
                    <Detail label="Trace" value={log.traceId} />
                  </div>

                  <div className="copy-row">
                    <CopyBlock
                      icon={<TerminalSquare size={14} />}
                      isCopied={copiedKey === `kubectl-${log.id}`}
                      label="kubectl"
                      onCopy={() => onCopy(`kubectl-${log.id}`, kubectlCommand)}
                      value={kubectlCommand}
                    />
                    <CopyBlock
                      icon={<FileJson2 size={14} />}
                      isCopied={copiedKey === `json-${log.id}`}
                      label="event json"
                      onCopy={() => onCopy(`json-${log.id}`, eventJson)}
                      value={eventJson}
                    />
                  </div>
                </div>
              ) : null}
            </article>
          );
        })}

        {logs.length === 0 ? <div className="empty-state">No logs match the current filters.</div> : null}
      </div>
    </section>
  );
}

type DetailProps = {
  label: string;
  value: string;
};

function Detail({ label, value }: DetailProps) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

type CopyBlockProps = {
  icon: React.ReactNode;
  isCopied: boolean;
  label: string;
  onCopy: () => void;
  value: string;
};

function CopyBlock({ icon, isCopied, label, onCopy, value }: CopyBlockProps) {
  return (
    <section className="copy-block">
      <header>
        <span aria-hidden="true">{icon}</span>
        <h3>{label}</h3>
        <button
          onClick={(event) => {
            event.stopPropagation();
            onCopy();
          }}
          type="button"
        >
          <Copy size={13} aria-hidden="true" />
          {isCopied ? "copied" : "copy"}
        </button>
      </header>
      <pre>{value}</pre>
    </section>
  );
}
