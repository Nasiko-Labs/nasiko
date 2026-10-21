/**
 * Policy presets for the two demo modes.
 *
 * These are ordinary Nasiko-style tool rules — the exact shape you would set in
 * the real MCP gateway's per-agent permission UI. Nothing here is hardcoded
 * "blocking logic"; it is declarative policy that the {@link PolicyEngine}
 * evaluates identically to Nasiko's `PermissionContext`.
 */
import type { PolicyConfig, RunMode } from "../shared/types.js";

/** No governance: connector enabled, every tool allowed. */
export const UNPROTECTED_POLICY: PolicyConfig = {
  label: "Unprotected (no governance)",
  enabledConnector: true,
  rules: [{ pattern: "*", stance: "allow" }],
};

/**
 * Nasiko research-agent policy. Read is fine; secrets, destruction, shell, and
 * config writes are blocked; broad customer reads and outbound email require
 * human approval (`ask`). Priority block>ask>allow means specific blocks win
 * over the broad allow.
 */
export const NASIKO_PROTECTED_POLICY: PolicyConfig = {
  label: "Nasiko — research-agent policy",
  enabledConnector: true,
  rules: [
    { pattern: "*", stance: "allow" },
    { pattern: "web.search", stance: "allow" },
    { pattern: "github.search", stance: "allow" },
    { pattern: "filesystem.read", stance: "ask" },
    { pattern: "filesystem.write", stance: "block" },
    { pattern: "secrets.read", stance: "block" },
    { pattern: "database.query", stance: "allow" },
    { pattern: "database.delete", stance: "block" },
    { pattern: "email.send", stance: "ask" },
    { pattern: "shell.execute", stance: "block" },
  ],
};

export function policyForMode(mode: RunMode): PolicyConfig {
  return mode === "protected" ? NASIKO_PROTECTED_POLICY : UNPROTECTED_POLICY;
}
