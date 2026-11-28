/**
 * Attack Runner — the orchestration seam that ties everything together:
 *
 *     step → PolicyLayer.decide() → allow/ask/block
 *          → (allow) Honeypot.execute()
 *          → SecurityEvent + risk
 *          → threat detection
 *
 * The runner is the ONLY place that consults policy. The honeypot never sees the
 * decision. This is what makes the integration real rather than hardcoded: swap
 * {@link PolicyLayer} for the Nasiko MCP gateway and nothing else changes.
 */
import { randomUUID } from "node:crypto";
import { HONEYPOT_TOOLS } from "./honeypot.js";
import { detectThreats, riskForAction, severityForScore } from "./risk.js";
import type {
  ApprovalRequest,
  AttackScenario,
  PolicyDecision,
  RunMode,
  RunResult,
  RunTotals,
  SecurityEvent,
} from "../shared/types.js";

/** The policy abstraction the runner depends on. Nasiko is one implementation. */
export interface PolicyLayer {
  readonly label: string;
  /**
   * Decide (and, for the real gateway, potentially execute) a tool call.
   * Returns the decision plus, when the gateway itself forwarded the call, the
   * result it got back — so the runner doesn't double-execute. In-process
   * policy returns no `gatewayResult`; the runner then executes locally.
   */
  decide(
    tool: string,
    args: Record<string, unknown>,
  ): PolicyDecision | Promise<PolicyDecision>;
}

export interface RunHooks {
  onEvent?: (e: SecurityEvent) => void;
  /**
   * Called when a tool resolves to `ask`. Return true to approve (execute) or
   * false to reject. If omitted, `ask` defaults to pending→rejected.
   */
  onApproval?: (req: ApprovalRequest) => Promise<boolean>;
}

const AGENT = "research-agent";

export async function runAttack(
  scenario: AttackScenario,
  mode: RunMode,
  policy: PolicyLayer,
  hooks: RunHooks = {},
): Promise<RunResult> {
  const runId = randomUUID();
  const startedAt = new Date().toISOString();
  const events: SecurityEvent[] = [];
  let riskScore = 0;
  let seq = 0;

  for (const step of scenario.steps) {
    seq += 1;
    const decision = await policy.decide(step.tool, step.args);
    let outcome: SecurityEvent["outcome"];

    if (decision.access === "allowed") {
      outcome = "executed";
    } else if (decision.access === "denied") {
      outcome = "blocked";
    } else {
      // ask → consult human approval hook
      const approved = hooks.onApproval
        ? await hooks.onApproval({
            eventId: `${runId}:${seq}`,
            runId,
            tool: step.tool,
            args: step.args,
            reason: decision.reason,
            severity: "high",
          })
        : false;
      outcome = approved ? "approved" : "rejected";
    }

    // Only actions that actually run reach the honeypot.
    const executed = outcome === "executed" || outcome === "approved";
    const exec = executed ? HONEYPOT_TOOLS[step.tool]?.(step.args) : undefined;

    const riskDelta = executed ? riskForAction(step.tool, step.args) : 0;
    riskScore = Math.min(100, riskScore + riskDelta);

    const event: SecurityEvent = {
      id: `${runId}:${seq}`,
      seq,
      timestamp: new Date().toISOString(),
      runId,
      mode,
      agent: AGENT,
      tool: step.tool,
      args: step.args,
      decision,
      outcome,
      riskDelta,
      riskScore,
      severity: exec?.severity ?? severityForScore(riskDelta),
      telemetry: {
        intent: step.intent,
        ...(exec?.telemetry ?? {}),
        ...(exec
          ? {
              touchedSensitiveData: exec.touchedSensitiveData,
              touchedSecret: exec.touchedSecret,
              external: exec.externalTransmission,
              containsSecrets: (exec.telemetry as any)?.containsSecrets,
            }
          : {}),
        ...(exec ? { result: exec.result } : {}),
      },
    };

    events.push(event);
    hooks.onEvent?.(event);
  }

  const threats = detectThreats(events);
  const totals = summarize(events);
  const finalScore = Math.min(100, riskScore);

  return {
    runId,
    mode,
    scenarioId: scenario.id,
    agent: AGENT,
    events,
    threats,
    riskScore: finalScore,
    severity: severityForScore(finalScore),
    totals,
    startedAt,
    finishedAt: new Date().toISOString(),
  };
}

function summarize(events: SecurityEvent[]): RunTotals {
  const executed = events.filter(
    (e) => e.outcome === "executed" || e.outcome === "approved",
  ).length;
  const blocked = events.filter(
    (e) => e.outcome === "blocked" || e.outcome === "rejected",
  ).length;
  const ask = events.filter((e) => e.decision.access === "ask").length;
  const suspicious = events.filter(
    (e) => e.severity === "high" || e.severity === "critical",
  ).length;
  return {
    toolCalls: events.length,
    executed,
    blocked,
    ask,
    suspicious,
  };
}
