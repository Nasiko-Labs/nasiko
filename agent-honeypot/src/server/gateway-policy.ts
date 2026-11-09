/**
 * GatewayPolicyLayer — the DEEP integration.
 *
 * Implements the runner's {@link PolicyLayer} by talking to the REAL Nasiko MCP
 * Gateway. Instead of deciding allow/ask/block in-process, it lets Nasiko's
 * `permissions.rs` decide — for real — on every call:
 *
 *   1. mint a delegation token (HS256, aud="mcp", act=agentId) with JWT_SECRET
 *      — byte-for-byte what `nasiko_auth::jwt::mint_delegation_token` produces.
 *   2. POST /api/mcp  { jsonrpc, method:"tools/call",
 *                       params:{ name:`${prefix}__${tool}`, arguments } }
 *      with header  x-nasiko-agent-token: <delegation token>.
 *   3. map the JSON-RPC response:
 *        - result (no error)        → ALLOWED  (gateway already forwarded to
 *                                     the honeypot MCP server)
 *        - error.code TOOL_BLOCKED  → DENIED
 *        - error.code TOOL_ASK      → ASK
 *        - other error              → DENIED (surface the message)
 *
 * The decision is made by Nasiko, not by us. This is what "governed by Nasiko"
 * means literally rather than by analogy.
 */
import jwt from "jsonwebtoken";
import type { PolicyDecision, ToolAccess } from "../shared/types.js";
import type { PolicyLayer } from "./runner.js";
import type { NasikoWiring } from "./nasiko-setup.js";

// Mirrors mcp-gateway/src/types.rs `codes`.
const TOOL_BLOCKED = -32000;
const TOOL_ASK = -32001;

const NASIKO_BASE = process.env.NASIKO_BASE_URL ?? "http://localhost:8080";
const JWT_SECRET = process.env.JWT_SECRET ?? "";
// Mirrors auth/src/jwt.rs DELEGATION_EXPIRY_SECS (5 min).
const DELEGATION_EXPIRY_SECS = 5 * 60;

/** Mint the exact delegation JWT the gateway's `require_delegation` accepts. */
export function mintDelegationToken(userId: string, agentId: string): string {
  if (!JWT_SECRET) throw new Error("JWT_SECRET is not set (see nasiko/.env).");
  const now = Math.floor(Date.now() / 1000);
  return jwt.sign(
    {
      sub: userId,
      act: agentId,
      aud: "mcp",
      iat: now,
      exp: now + DELEGATION_EXPIRY_SECS,
    },
    JWT_SECRET,
    { algorithm: "HS256" },
  );
}

export class GatewayPolicyLayer implements PolicyLayer {
  readonly label = "Nasiko MCP Gateway (live permissions.rs)";
  private readonly token: string;

  constructor(private readonly wiring: NasikoWiring) {
    this.token = mintDelegationToken(wiring.userId, wiring.agentId);
  }

  async decide(
    tool: string,
    args: Record<string, unknown>,
  ): Promise<PolicyDecision> {
    const namespaced = `${this.wiring.prefix}__${tool}`;
    let body: any;
    try {
      const res = await fetch(`${NASIKO_BASE}/api/mcp`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "x-nasiko-agent-token": this.token,
        },
        body: JSON.stringify({
          jsonrpc: "2.0",
          id: 1,
          method: "tools/call",
          params: { name: namespaced, arguments: args },
        }),
      });
      body = await res.json();
    } catch (e) {
      return {
        access: "denied",
        stance: "block",
        reason: `Gateway unreachable: ${(e as Error).message}`,
        policyLabel: this.label,
      };
    }

    const errCode: number | undefined = body?.error?.code;
    const errMsg: string = body?.error?.message ?? "";

    let access: ToolAccess;
    let stance: PolicyDecision["stance"];
    let reason: string;

    if (errCode === TOOL_BLOCKED) {
      access = "denied";
      stance = "block";
      reason =
        errMsg || `Nasiko blocked "${tool}" (permissions.rs stance=block).`;
    } else if (errCode === TOOL_ASK) {
      access = "ask";
      stance = "ask";
      reason = errMsg || `Nasiko requires approval for "${tool}" (stance=ask).`;
    } else if (body?.error) {
      access = "denied";
      stance = "block";
      reason = `Nasiko rejected "${tool}": ${errMsg}`;
    } else {
      access = "allowed";
      stance = "allow";
      reason = `Nasiko allowed "${tool}" (stance=allow); gateway forwarded the call to the honeypot.`;
    }

    return { access, stance, reason, policyLabel: this.label };
  }
}
