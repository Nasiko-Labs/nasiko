/**
 * Shared types for Agent Honeypot.
 *
 * The policy vocabulary here is a faithful mirror of Nasiko's real MCP-gateway
 * permission model (`mcp-gateway/src/types.rs` + `permissions.rs`):
 *
 *   - Stance      = allow | ask | block          (priority: block > ask > allow)
 *   - ToolAccess  = Allowed | Ask | Denied        (the resolved decision)
 *
 * Keeping these identical is deliberate: the honeypot's policy layer is a
 * drop-in stand-in for Nasiko's `PermissionContext::decide()`, so the real
 * gateway can replace it later without changing anything downstream.
 */

/** Per-tool permission stance. Mirrors Nasiko `Stance`. */
export type Stance = "allow" | "ask" | "block";

/** Resolved access decision. Mirrors Nasiko `ToolAccess`. */
export type ToolAccess = "allowed" | "ask" | "denied";

/** Severity ladder used by the risk engine and threat findings. */
export type Severity = "low" | "medium" | "high" | "critical";

/** How a tool call ultimately resolved once policy + (optional) human ran. */
export type Outcome =
  | "executed" // ran in the sandbox
  | "blocked" // policy denied it
  | "ask" // policy requires human approval (pending)
  | "approved" // human approved an `ask`
  | "rejected"; // human rejected an `ask`

/** A single glob rule: pattern → stance. Mirrors Nasiko `ToolRule`. */
export interface ToolRule {
  pattern: string;
  stance: Stance;
}

/**
 * The policy configuration for one run. `enabledConnector` mirrors Nasiko's
 * default-deny connector toggle: when false, EVERY tool is denied regardless of
 * rules (exactly like `PermissionContext::decide`'s connector-enable check).
 */
export interface PolicyConfig {
  /** Human label for the policy shown in the dashboard. */
  label: string;
  /** Default-deny connector toggle. false ⇒ deny everything. */
  enabledConnector: boolean;
  /** Per-tool glob rules. Default (no match) is `allow`. */
  rules: ToolRule[];
}

/** The result of a policy evaluation for one tool call. */
export interface PolicyDecision {
  access: ToolAccess;
  stance: Stance;
  /** Human-readable justification shown in "Why was this blocked?". */
  reason: string;
  /** The policy label that produced this decision. */
  policyLabel: string;
}

/** Structured security telemetry emitted for every tool call. */
export interface SecurityEvent {
  id: string;
  seq: number;
  timestamp: string;
  runId: string;
  mode: RunMode;
  agent: string;
  tool: string;
  args: Record<string, unknown>;
  /** Resolved policy decision (from the policy layer). */
  decision: PolicyDecision;
  /** Final outcome after policy (+ optional human approval). */
  outcome: Outcome;
  /** Risk points this action contributed. */
  riskDelta: number;
  /** Running risk score after this action. */
  riskScore: number;
  severity: Severity;
  /** Tool-specific structured telemetry (fake, isolated). */
  telemetry: Record<string, unknown>;
}

/** A correlated multi-step finding (e.g. exfiltration). */
export interface ThreatFinding {
  id: string;
  type: string;
  title: string;
  severity: Severity;
  evidence: string[];
  /** Event seqs that make up this finding. */
  relatedSeqs: number[];
}

export type RunMode = "unprotected" | "protected" | "gateway";

/** A deterministic step in an attack scenario. */
export interface AttackStep {
  tool: string;
  args: Record<string, unknown>;
  /** Optional human-readable intent shown in the timeline. */
  intent?: string;
}

export interface AttackScenario {
  id: string;
  name: string;
  description: string;
  steps: AttackStep[];
}

/** A completed (or in-progress) attack run. */
export interface RunResult {
  runId: string;
  mode: RunMode;
  scenarioId: string;
  agent: string;
  events: SecurityEvent[];
  threats: ThreatFinding[];
  riskScore: number;
  severity: Severity;
  totals: RunTotals;
  startedAt: string;
  finishedAt?: string;
}

export interface RunTotals {
  toolCalls: number;
  executed: number;
  blocked: number;
  ask: number;
  suspicious: number;
}

/** A pending human-approval request (Nasiko `ask` stance). */
export interface ApprovalRequest {
  eventId: string;
  runId: string;
  tool: string;
  args: Record<string, unknown>;
  reason: string;
  severity: Severity;
}
