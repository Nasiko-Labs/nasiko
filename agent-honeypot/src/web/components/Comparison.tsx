import type { RunMode, RunResult } from "../../shared/types.js";

export function Comparison({
  runs,
}: {
  runs: Record<RunMode, RunResult | null>;
}) {
  const u = runs.unprotected;
  // Prefer the LIVE gateway run as the "protected" side when present — it's the
  // hero (real permissions.rs). Fall back to the in-process port run.
  const p = runs.gateway ?? runs.protected;
  const governedTitle = runs.gateway
    ? "WITH NASIKO ⚡ (live gateway)"
    : "WITH NASIKO";
  if (!u && !p) return null;

  return (
    <section className="mt-5 rounded-lg border border-edge bg-panel p-4">
      <div className="text-xs uppercase tracking-wider text-slate-400 mb-3">
        Run Comparison · same agent, same attack
      </div>
      <div className="grid grid-cols-2 gap-4">
        <Column title="WITHOUT NASIKO" color="text-critical" run={u} />
        <Column title={governedTitle} color="text-accent" run={p} />
      </div>
      {u && p && (
        <div className="mt-4 text-center text-sm">
          <span className="text-slate-400">Dangerous actions executed: </span>
          <span className="text-critical font-bold">
            {u.totals.executed - benign(u)}
          </span>
          <span className="text-slate-500"> → </span>
          <span className="text-low font-bold">
            {p.totals.executed - benign(p)}
          </span>
          <span className="text-slate-400"> · Risk </span>
          <span className="text-critical font-bold">{u.riskScore}</span>
          <span className="text-slate-500"> → </span>
          <span className="text-low font-bold">{p.riskScore}</span>
        </div>
      )}
    </section>
  );
}

/** Benign tool calls (web/github search) don't count as "dangerous". */
function benign(r: RunResult): number {
  return r.events.filter(
    (e) =>
      (e.tool === "web.search" || e.tool === "github.search") &&
      (e.outcome === "executed" || e.outcome === "approved"),
  ).length;
}

function Column({
  title,
  color,
  run,
}: {
  title: string;
  color: string;
  run: RunResult | null;
}) {
  return (
    <div className="rounded border border-edge bg-panel2 p-4">
      <div className={`font-bold text-sm mb-3 ${color}`}>{title}</div>
      {!run ? (
        <div className="text-slate-600 text-sm">Not run yet.</div>
      ) : (
        <ul className="space-y-1 text-sm text-slate-300">
          <li>{run.totals.toolCalls} tool calls</li>
          <li className="text-low">{run.totals.executed} executed</li>
          <li className="text-critical">{run.totals.blocked} blocked</li>
          <li className="text-medium">{run.totals.ask} required approval</li>
          <li className="text-high">{run.threats.length} threats detected</li>
        </ul>
      )}
    </div>
  );
}
