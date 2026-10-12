/**
 * Policy engine — a faithful stand-in for Nasiko's MCP-gateway permission model.
 *
 * This is a direct behavioral port of `mcp-gateway/src/permissions.rs`:
 *
 *   PermissionContext::decide(connector, tool):
 *     1. connector-enable toggle FIRST — a disabled connector denies EVERY tool
 *        regardless of any allow rule (default-deny).
 *     2. then per-tool stance: block → Denied, ask → Ask, allow → Allowed.
 *
 *   get_stance(tool):
 *     - case-insensitive glob match (`*`, `GMAIL_*`, exact)
 *     - priority: block > ask > allow
 *     - default (no rule matches): allow
 *
 * Keeping this isolated behind {@link PolicyEngine} enforces the brief's
 * architectural rule: the honeypot NEVER decides allow/block — the policy layer
 * does. Swapping in the real Nasiko gateway means replacing this one class.
 */
import type {
  PolicyConfig,
  PolicyDecision,
  Stance,
  ToolAccess,
  ToolRule,
} from "../shared/types.js";

/**
 * Case-insensitive glob match supporting `*` wildcards, mirroring Nasiko's
 * `wildcard_match`. `*` matches any run of characters (including empty).
 */
export function wildcardMatch(pattern: string, value: string): boolean {
  const p = pattern.toLowerCase();
  const v = value.toLowerCase();
  // Convert glob to a RegExp: escape everything, then turn \* into .*
  const escaped = p.replace(/[.+?^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*");
  return new RegExp(`^${escaped}$`).test(v);
}

const STANCE_PRIORITY: Stance[] = ["block", "ask", "allow"];

export class PolicyEngine {
  constructor(private readonly config: PolicyConfig) {}

  get label(): string {
    return this.config.label;
  }

  /** Resolve the stance for a tool. Priority block>ask>allow; default allow. */
  getStance(tool: string): Stance {
    const matching: Stance[] = this.config.rules
      .filter((r: ToolRule) => wildcardMatch(r.pattern, tool))
      .map((r) => r.stance);

    if (matching.length === 0) return "allow";
    for (const s of STANCE_PRIORITY) {
      if (matching.includes(s)) return s;
    }
    return "allow";
  }

  /** The full access decision. Connector-enable toggle first, then stance. */
  decide(tool: string, _args?: Record<string, unknown>): PolicyDecision {
    if (!this.config.enabledConnector) {
      return {
        access: "denied",
        stance: "block",
        reason:
          "Connector is disabled for this agent (default-deny). No tools may run until it is explicitly enabled.",
        policyLabel: this.config.label,
      };
    }

    const stance = this.getStance(tool);
    const access: ToolAccess =
      stance === "block" ? "denied" : stance === "ask" ? "ask" : "allowed";

    return {
      access,
      stance,
      reason: this.reasonFor(tool, stance),
      policyLabel: this.config.label,
    };
  }

  /** Human-readable justification shown in the dashboard's "Why?" panel. */
  private reasonFor(tool: string, stance: Stance): string {
    if (stance === "allow") {
      return `Tool "${tool}" is permitted for this agent under policy "${this.config.label}".`;
    }
    const rule = this.config.rules.find(
      (r) => wildcardMatch(r.pattern, tool) && r.stance === stance,
    );
    const via = rule ? ` (rule: ${rule.pattern} → ${stance})` : "";
    return POLICY_REASONS[tool]?.[stance] ?? genericReason(tool, stance, via);
  }
}

function genericReason(tool: string, stance: Stance, via: string): string {
  if (stance === "block") return `Tool "${tool}" is blocked by policy${via}.`;
  return `Tool "${tool}" requires human approval before it can run${via}.`;
}

/**
 * Curated, demo-quality explanations per (tool, stance). Falls back to a
 * generic message for anything not listed.
 */
const POLICY_REASONS: Record<string, Partial<Record<Stance, string>>> = {
  "secrets.read": {
    block:
      "Agents may not read credentials. Secret material is off-limits to autonomous agents under the research-agent policy.",
    ask: "Reading secrets requires human approval — a reviewer must confirm the agent's need before access is granted.",
  },
  "database.delete": {
    block:
      "Destructive database operations are not permitted for research agents. This agent has read-only database access.",
    ask: "Destructive database operations require explicit human approval before execution.",
  },
  "database.query": {
    ask: "Broad customer-data reads require human approval to prevent bulk exfiltration.",
    block: "Direct database reads are blocked for this agent.",
  },
  "email.send": {
    ask: "Outbound email to external recipients requires human approval — it may exfiltrate sensitive data.",
    block:
      "Sending external email is blocked: this agent has no communication permissions.",
  },
  "filesystem.write": {
    block:
      "Filesystem writes are blocked — the agent may not modify configuration, policies, or executables.",
    ask: "Filesystem writes require human approval.",
  },
  "filesystem.read": {
    ask: "Reading sensitive paths (e.g. .env) requires human approval.",
    block: "Filesystem reads are blocked for this agent.",
  },
  "shell.execute": {
    block:
      "Shell execution is blocked — arbitrary command execution is never permitted for research agents.",
    ask: "Shell execution requires human approval.",
  },
};
