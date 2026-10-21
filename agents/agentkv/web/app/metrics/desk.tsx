"use client";

import { useEffect, useMemo, useState } from "react";
import {
  METHODS,
  type Method,
  type Report,
  type TrialRow,
  type WorkflowRecord,
} from "@/lib/report";

const NASIKO = process.env.NEXT_PUBLIC_NASIKO_URL;

function format(value: number | null | undefined, suffix = "") {
  return value == null ? "—" : `${value.toFixed(2)}${suffix}`;
}

function lastStatus(record: WorkflowRecord, agent: string) {
  const events = (record.events || []).filter((event) => event.agent === agent);
  return events.length ? events[events.length - 1].status : "Not observed";
}

export function MetricsDesk() {
  const [report, setReport] = useState<Report | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [recordIndex, setRecordIndex] = useState(0);
  const [cohort, setCohort] = useState<number | null>(null);

  useEffect(() => {
    fetch("/evidence.json")
      .then((response) => {
        if (!response.ok) throw new Error("Evidence file unavailable");
        return response.json() as Promise<Report>;
      })
      .then((data) => {
        setReport(data);
        const first = data.rows[0]?.concurrency;
        setCohort(first ?? null);
      })
      .catch((err: Error) => setError(err.message));
  }, []);

  const record = report?.records[recordIndex];
  const row = useMemo(() => {
    if (!report || !record) return null;
    return report.rows.find(
      (trial) =>
        trial.method === record.method &&
        trial.repetition === record.repetition &&
        (record.concurrency == null || record.concurrency === trial.concurrency),
    );
  }, [report, record]);

  const cohorts = useMemo(() => {
    if (!report) return [];
    return [...new Set(report.rows.map((trial) => trial.concurrency))].sort((a, b) => a - b);
  }, [report]);

  const cohortRows = useMemo(() => {
    if (!report || cohort == null) return [];
    return report.rows.filter((trial) => trial.concurrency === cohort);
  }, [report, cohort]);

  if (error) {
    return (
      <main className="px-[min(6vw,72px)] py-24">
        <h1 className="font-display text-5xl">Evidence unavailable.</h1>
        <p className="mt-4 text-mute">{error}</p>
      </main>
    );
  }

  if (!report || !record) {
    return (
      <main className="px-[min(6vw,72px)] py-24">
        <p className="font-mono text-[11px] uppercase tracking-[0.28em] text-mute">Loading recorded evidence</p>
      </main>
    );
  }

  const acks = record.engine_acknowledgements || [];
  const prioritized = acks.filter((ack) => ack.status === "prioritized").length;

  return (
    <main className="px-[min(6vw,72px)] pb-20 pt-12">
      <div className="flex flex-col justify-between gap-8 md:flex-row md:items-end">
        <div>
          <p className="font-mono text-[11px] uppercase tracking-[0.28em] text-mute">
            Nasiko control plane · recorded companion
          </p>
          <h1 className="mt-4 font-display text-[clamp(2.6rem,6vw,4.6rem)] leading-[0.92] tracking-[-0.04em]">
            See where reuse helps.
          </h1>
          <p className="mt-5 max-w-xl text-mute">
            Follow the agents. Inspect each cache decision. Compare what the same work
            costs, and how long it takes. Nothing here is simulated.
          </p>
        </div>
        <div className="text-right font-mono text-xs leading-7 text-mute">
          <div>{report.model} / BF16</div>
          <div>{report.gpu} · Modal</div>
          {NASIKO ? (
            <a className="border-b border-ink text-ink" href={NASIKO}>
              Nasiko live ↗
            </a>
          ) : (
            <span>Nasiko environment stopped</span>
          )}
          <div className="mt-2 uppercase tracking-[0.18em] text-ink">
            {report.status === "recorded" ? "Recorded benchmark" : "Awaiting benchmark"}
          </div>
        </div>
      </div>

      <div className="mt-10 grid gap-px bg-rule sm:grid-cols-2 lg:grid-cols-4">
        <Kpi label="Completed tests" value={row ? `${row.successes} / ${row.tasks}` : "—"} note="Successful / attempted workflows" />
        <Kpi label="Workflow latency" value={row ? format(row.p95_seconds, "s") : "—"} note="p95 · selected trial" />
        <Kpi
          label="Cost per 1,000 successes"
          value={row?.cost_per_thousand == null ? "—" : `$${row.cost_per_thousand.toFixed(2)}`}
          note={row?.cost_per_thousand == null ? "Incomplete cost evidence" : "Active resource window · estimate"}
        />
        <Kpi label="Engine-confirmed actions" value={String(prioritized)} note="Eviction priority, not a hard pin" />
      </div>

      <div className="mt-8 grid gap-px bg-rule lg:grid-cols-[1.35fr_1fr]">
        <section className="bg-cream p-6 md:p-8">
          <div className="flex flex-wrap items-center justify-between gap-4">
            <h2 className="font-display text-2xl tracking-[-0.03em]">01 / Agent execution</h2>
            <select
              aria-label="Select recorded workflow"
              className="max-w-[min(100%,280px)] border border-rule bg-transparent px-3 py-2 font-mono text-xs"
              value={recordIndex}
              onChange={(event) => setRecordIndex(Number(event.target.value))}
            >
              {report.records.map((item, index) => (
                <option key={`${item.method}-${item.task_id}-${item.repetition}-${index}`} value={index}>
                  {item.method} / {item.task_id} / c{item.concurrency || 1} · r{item.repetition + 1}
                </option>
              ))}
            </select>
          </div>
          <div className="mt-6 grid grid-cols-3 gap-2">
            {(["coder", "tester", "reviewer"] as const).map((agent, index) => {
              const status = lastStatus(record, agent);
              const done = (record.events || []).some((event) => event.agent === agent && event.status === "completed");
              return (
                <div
                  key={agent}
                  className={`border p-4 ${done ? "border-ink border-t-[3px]" : "border-rule"}`}
                >
                  <span className="font-mono text-[10px] uppercase tracking-[0.2em] text-mute">0{index + 1}</span>
                  <strong className="mt-3 block capitalize">{agent}</strong>
                  <small className="font-mono text-[11px] text-mute">{status}</small>
                </div>
              );
            })}
          </div>
          <div className="mt-4 min-h-[140px]">
            {(record.events || []).length === 0 ? (
              <p className="px-3 py-10 text-center text-sm text-mute">
                Recorded agent events will appear here.
                <br />
                No simulated activity is shown.
              </p>
            ) : (
              (record.events || []).map((event, index) => (
                <div
                  key={`${event.agent}-${event.time}-${index}`}
                  className="grid grid-cols-[70px_1fr_110px] border-t border-rule py-3 text-xs"
                >
                  <span>{new Date(event.time * 1000).toLocaleTimeString([], { hour12: false })}</span>
                  <span>{event.agent}</span>
                  <span className="text-right font-mono">{event.status}</span>
                </div>
              ))
            )}
          </div>
          <div className="mt-6 bg-forest p-6 text-cream">
            <p className="font-mono text-[10px] uppercase tracking-[0.24em] text-cream/60">02 / Decision inspector</p>
            <h3 className="mt-3 font-display text-2xl text-phosphor">
              {record.hints?.length ? "Eviction preference requested" : "Stock cache behavior"}
            </h3>
            <p className="mt-3 text-sm text-cream/70">
              {record.error ||
                `Inspect the engine response before attributing any benefit to the policy. Completed tests: ${record.success ? "pass" : "fail"}`}
            </p>
            <dl className="mt-6 grid gap-4 sm:grid-cols-2">
              <Fact label="Decision source" value={`${record.method} · ${METHODS[record.method].name}`} />
              <Fact
                label="Near-term repair estimate"
                value={
                  record.repair_probability == null
                    ? "Not requested"
                    : `${Math.round(record.repair_probability * 100)}%${record.method === "E" ? " · uncalibrated" : ""}`
                }
              />
              <Fact
                label="Requested lease"
                value={record.hints?.length ? `${record.hints[0].lease_seconds} seconds` : "None"}
              />
              <Fact
                label="Engine response"
                value={acks.length ? acks.map((ack) => ack.status).join(", ") : "No acknowledgement"}
              />
              <Fact
                label="Proposal timing"
                value={
                  record.decision_after_test === true
                    ? "After test result"
                    : record.decision_after_test === false
                      ? "Before test result"
                      : "Not measured"
                }
              />
              <Fact
                label="Observed resident group"
                value={
                  record.policy_cost?.resident_bytes != null
                    ? format(record.policy_cost.resident_bytes / 1048576, " MiB")
                    : "Not measured"
                }
              />
            </dl>
          </div>
        </section>

        <section className="bg-cream p-6 md:p-8">
          <div className="flex items-center justify-between gap-4">
            <h2 className="font-display text-2xl tracking-[-0.03em]">03 / Cost & latency</h2>
            <select
              aria-label="Concurrency cohort"
              className="border border-rule bg-transparent px-3 py-2 font-mono text-xs"
              value={cohort ?? ""}
              onChange={(event) => setCohort(Number(event.target.value))}
            >
              {cohorts.map((value) => (
                <option key={value} value={value}>
                  Concurrency {value}
                </option>
              ))}
            </select>
          </div>
          <FrontierChart rows={cohortRows} />
          <div className="mt-4 flex flex-wrap gap-x-4 gap-y-2 font-mono text-[11px] leading-5">
            {(Object.keys(METHODS) as Method[]).map((method) => (
              <span key={method} className="flex items-center gap-2">
                <i className="inline-block h-2 w-2 rounded-full" style={{ background: METHODS[method].color }} />
                {method} · {METHODS[method].name}
              </span>
            ))}
          </div>
          <p className="mt-4 text-sm leading-relaxed text-mute">
            Each point is an observed trial; the dashed line connects non-dominated observations.
            Only runs in the selected cohort with all task tests passing and complete cost evidence qualify.
            This pilot frontier has no statistical superiority guarantee.
          </p>
          <p className="mt-3 text-sm leading-relaxed text-mute">{(report.notes || []).join(" ")}</p>
        </section>
      </div>

      <div className="mt-8 overflow-x-auto border border-rule">
        <table className="w-full border-collapse text-left text-xs">
          <thead>
            <tr className="font-mono text-[10px] uppercase tracking-[0.16em] text-mute">
              <th className="border-b border-rule px-3 py-4">Method</th>
              <th className="border-b border-rule px-3 py-4">Concurrency / repeat</th>
              <th className="border-b border-rule px-3 py-4">Passed</th>
              <th className="border-b border-rule px-3 py-4">Cached tokens</th>
              <th className="border-b border-rule px-3 py-4">p50</th>
              <th className="border-b border-rule px-3 py-4">p95</th>
              <th className="border-b border-rule px-3 py-4">Successes/hour</th>
              <th className="border-b border-rule px-3 py-4">$/1,000 successes</th>
            </tr>
          </thead>
          <tbody>
            {report.rows.map((trial, index) => (
              <tr key={`${trial.method}-${trial.concurrency}-${trial.repetition}-${index}`} className="hover:bg-paper">
                <td className="border-b border-rule px-3 py-4">
                  {trial.method} · {METHODS[trial.method].name}
                </td>
                <td className="border-b border-rule px-3 py-4">
                  {trial.concurrency} / {trial.repetition + 1}
                </td>
                <td className="border-b border-rule px-3 py-4">
                  {trial.successes}/{trial.tasks}
                </td>
                <td className="border-b border-rule px-3 py-4">
                  {trial.cache_usage.cached_fraction == null
                    ? "Unavailable"
                    : `${(trial.cache_usage.cached_fraction * 100).toFixed(1)}%`}
                </td>
                <td className="border-b border-rule px-3 py-4">{format(trial.p50_seconds, "s")}</td>
                <td className="border-b border-rule px-3 py-4">{format(trial.p95_seconds, "s")}</td>
                <td className="border-b border-rule px-3 py-4">{format(trial.successes_per_hour)}</td>
                <td className="border-b border-rule px-3 py-4">
                  {trial.cost_per_thousand == null ? "Unavailable" : `$${format(trial.cost_per_thousand)}`}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </main>
  );
}

function Kpi({ label, value, note }: { label: string; value: string; note: string }) {
  return (
    <div className="bg-cream px-6 py-7">
      <p className="font-mono text-[10px] uppercase tracking-[0.22em] text-mute">{label}</p>
      <p className="mt-4 font-display text-4xl tracking-[-0.04em]">{value}</p>
      <p className="mt-2 text-sm text-mute">{note}</p>
    </div>
  );
}

function Fact({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt className="text-[11px] text-cream/55">{label}</dt>
      <dd className="mt-1 font-mono text-[13px]">{value}</dd>
    </div>
  );
}

function FrontierChart({ rows }: { rows: TrialRow[] }) {
  const points = rows.filter(
    (row) =>
      row.cost_per_thousand != null &&
      row.p95_seconds != null &&
      row.tasks &&
      row.successes === row.tasks,
  );
  if (!points.length) {
    return (
      <p className="mt-10 px-4 py-16 text-center text-sm text-mute">
        No quality-qualified points with complete cost evidence in this cohort.
      </p>
    );
  }
  const maxX = Math.max(...points.map((point) => point.p95_seconds as number)) * 1.15;
  const maxY = Math.max(...points.map((point) => point.cost_per_thousand as number)) * 1.15;
  const frontier = points
    .filter(
      (point) =>
        !points.some(
          (other) =>
            other !== point &&
            (other.p95_seconds as number) <= (point.p95_seconds as number) &&
            (other.cost_per_thousand as number) <= (point.cost_per_thousand as number) &&
            ((other.p95_seconds as number) < (point.p95_seconds as number) ||
              (other.cost_per_thousand as number) < (point.cost_per_thousand as number)),
        ),
    )
    .sort((a, b) => (a.p95_seconds as number) - (b.p95_seconds as number));

  return (
    <svg viewBox="0 0 520 300" role="img" aria-label="Observed cost versus p95 latency" className="mt-6 h-[310px] w-full overflow-visible">
      {[0, 1, 2, 3, 4].map((i) => {
        const y = 245 - i * 50;
        return (
          <g key={i}>
            <line x1="60" y1={y} x2="500" y2={y} stroke="#cbbd9a" />
            <text x="50" y={y + 4} textAnchor="end" fontSize="10" fill="#6b6454">
              ${((maxY * i) / 4).toFixed(1)}
            </text>
            <text x={60 + i * 110} y="266" textAnchor="middle" fontSize="10" fill="#6b6454">
              {((maxX * i) / 4).toFixed(1)}
            </text>
          </g>
        );
      })}
      <text x="280" y="290" textAnchor="middle" fontSize="11" fill="#6b6454">
        p95 workflow latency / seconds
      </text>
      {frontier.length > 1 && (
        <polyline
          fill="none"
          stroke="#12201c"
          strokeWidth="1.5"
          strokeDasharray="4 4"
          points={frontier
            .map(
              (point) =>
                `${60 + ((point.p95_seconds as number) / maxX) * 440},${245 - ((point.cost_per_thousand as number) / maxY) * 200}`,
            )
            .join(" ")}
        />
      )}
      {points.map((point, index) => (
        <circle
          key={`${point.method}-${point.repetition}-${index}`}
          cx={60 + ((point.p95_seconds as number) / maxX) * 440}
          cy={245 - ((point.cost_per_thousand as number) / maxY) * 200}
          r="6"
          fill={METHODS[point.method].color}
        >
          <title>
            {point.method}: {format(point.p95_seconds, "s")}, ${format(point.cost_per_thousand)}
          </title>
        </circle>
      ))}
    </svg>
  );
}
