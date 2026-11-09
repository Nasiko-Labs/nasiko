import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  ApprovalRequest,
  AttackScenario,
  RunMode,
  RunResult,
  SecurityEvent,
} from "../shared/types.js";
import {
  connectStream,
  fetchScenarios,
  gatewaySetup,
  respondApproval,
  runScenario,
} from "./api.js";
import { Metrics } from "./components/Metrics.js";
import { Timeline } from "./components/Timeline.js";
import { Threats } from "./components/Threats.js";
import { WhyPanel } from "./components/WhyPanel.js";
import { ApprovalModal } from "./components/ApprovalModal.js";
import { Comparison } from "./components/Comparison.js";

export default function App() {
  const [scenarios, setScenarios] = useState<AttackScenario[]>([]);
  const [tools, setTools] = useState<string[]>([]);
  const [scenarioId, setScenarioId] = useState("exfiltration");
  const [mode, setMode] = useState<RunMode>("unprotected");
  const [events, setEvents] = useState<SecurityEvent[]>([]);
  const [result, setResult] = useState<RunResult | null>(null);
  const [running, setRunning] = useState(false);
  const [selected, setSelected] = useState<SecurityEvent | null>(null);
  const [approval, setApproval] = useState<ApprovalRequest | null>(null);
  const [gatewayMsg, setGatewayMsg] = useState<string>("");
  const [runs, setRuns] = useState<Record<RunMode, RunResult | null>>({
    unprotected: null,
    protected: null,
    gateway: null,
  });
  const policyLabel = useRef<string>("");

  useEffect(() => {
    fetchScenarios().then((d) => {
      setScenarios(d.scenarios);
      setTools(d.tools);
    });
  }, []);

  useEffect(() => {
    return connectStream({
      onRunStart: (d) => {
        policyLabel.current = d.policy;
        setEvents([]);
        setResult(null);
        setSelected(null);
      },
      onEvent: (e) => setEvents((prev) => [...prev, e]),
      onApprovalRequest: (r) => setApproval(r),
      onRunComplete: (r) => {
        setResult(r);
        setRuns((prev) => ({ ...prev, [r.mode]: r }));
        setRunning(false);
        setApproval(null);
      },
    });
  }, []);

  const run = useCallback(
    async (m: RunMode) => {
      setMode(m);
      setRunning(true);
      await runScenario(scenarioId, m);
    },
    [scenarioId],
  );

  const onApprove = useCallback(
    async (approved: boolean) => {
      if (!approval) return;
      await respondApproval(approval.eventId, approved);
      setApproval(null);
    },
    [approval],
  );

  const runGateway = useCallback(async () => {
    setRunning(true);
    setGatewayMsg("Wiring honeypot into Nasiko gateway…");
    const setup = await gatewaySetup();
    if (!setup.ok) {
      setGatewayMsg(`Gateway setup failed: ${setup.error}`);
      setRunning(false);
      return;
    }
    setGatewayMsg(
      `Wired · connector ${setup.connectorId?.slice(0, 8)} · agent ${setup.agentId?.slice(0, 8)}`,
    );
    setMode("gateway");
    // Gateway `ask` is auto-rejected server-side for a clean deterministic demo.
    await runScenario(scenarioId, "gateway", false);
  }, [scenarioId]);

  const activeScenario = useMemo(
    () => scenarios.find((s) => s.id === scenarioId),
    [scenarios, scenarioId],
  );

  const liveRisk = events.at(-1)?.riskScore ?? 0;
  const totals = result?.totals ?? {
    toolCalls: events.length,
    executed: events.filter(
      (e) => e.outcome === "executed" || e.outcome === "approved",
    ).length,
    blocked: events.filter(
      (e) => e.outcome === "blocked" || e.outcome === "rejected",
    ).length,
    ask: events.filter((e) => e.decision.access === "ask").length,
    suspicious: events.filter(
      (e) => e.severity === "high" || e.severity === "critical",
    ).length,
  };

  return (
    <div className="min-h-full p-5 max-w-[1400px] mx-auto">
      {/* Header */}
      <header className="flex items-center justify-between mb-5">
        <div>
          <h1 className="text-2xl font-bold text-white tracking-tight">
            🐝 AGENT HONEYPOT
          </h1>
          <p className="text-xs text-slate-500 mt-1">
            An adversarial sandbox for AI agents ·{" "}
            <span className="text-accent">governed by Nasiko</span>
          </p>
        </div>
        <div className="flex items-center gap-3 text-xs">
          <span className="text-slate-500">Agent:</span>
          <span className="text-white font-semibold">research-agent</span>
          <span
            className={`flex items-center gap-1.5 ${running ? "text-medium" : "text-low"}`}
          >
            <span
              className={`w-2 h-2 rounded-full ${running ? "bg-medium pulse" : "bg-low"}`}
            />
            {running ? "RUNNING" : "IDLE"}
          </span>
        </div>
      </header>

      {/* Attack lab */}
      <section className="rounded-lg border border-edge bg-panel p-4 mb-5">
        <div className="flex flex-wrap items-center gap-4">
          <div className="flex items-center gap-2">
            <span className="text-xs text-slate-500 uppercase">Scenario</span>
            <select
              value={scenarioId}
              onChange={(e) => setScenarioId(e.target.value)}
              className="bg-panel2 border border-edge rounded px-3 py-1.5 text-sm text-white"
            >
              {scenarios.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name}
                </option>
              ))}
            </select>
          </div>
          <p className="text-xs text-slate-400 flex-1 min-w-[240px]">
            {activeScenario?.description}
          </p>
          <div className="flex items-center gap-2">
            <button
              disabled={running}
              onClick={() => run("unprotected")}
              className="px-4 py-2 rounded bg-critical/20 border border-critical/50 text-critical text-sm font-semibold hover:bg-critical/30 disabled:opacity-40"
            >
              ▶ Run Unprotected
            </button>
            <button
              disabled={running}
              onClick={() => run("protected")}
              className="px-4 py-2 rounded bg-accent/20 border border-accent/50 text-accent text-sm font-semibold hover:bg-accent/30 disabled:opacity-40"
            >
              🛡 Run with Nasiko
            </button>
            <button
              disabled={running}
              onClick={runGateway}
              title="Runs the attack through the LIVE Nasiko MCP Gateway (real permissions.rs)"
              className="px-4 py-2 rounded bg-low/20 border border-low/50 text-low text-sm font-semibold hover:bg-low/30 disabled:opacity-40"
            >
              ⚡ Run via Live Gateway
            </button>
          </div>
        </div>
        {gatewayMsg && (
          <div className="mt-2 text-[11px] text-low">⚡ {gatewayMsg}</div>
        )}
        <div className="mt-3 text-[11px] text-slate-500">
          Attack surface:{" "}
          <span className="text-slate-300">{tools.length} tools</span> · Active
          policy:{" "}
          <span className="text-slate-300">{policyLabel.current || "—"}</span> ·
          Mode:{" "}
          <span
            className={mode === "protected" ? "text-accent" : "text-critical"}
          >
            {mode.toUpperCase()}
          </span>
        </div>
      </section>

      <Metrics
        risk={result?.riskScore ?? liveRisk}
        totals={totals}
        threats={result?.threats.length ?? 0}
      />

      <div className="grid grid-cols-1 lg:grid-cols-3 gap-5 mt-5">
        <div className="lg:col-span-2">
          <Timeline
            events={events}
            onSelect={setSelected}
            selected={selected}
          />
        </div>
        <div className="space-y-5">
          <Threats threats={result?.threats ?? []} />
          {selected && <WhyPanel event={selected} />}
        </div>
      </div>

      <Comparison runs={runs} />

      {approval && <ApprovalModal request={approval} onDecide={onApprove} />}
    </div>
  );
}
