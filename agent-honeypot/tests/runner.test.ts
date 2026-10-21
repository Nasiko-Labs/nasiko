import { describe, expect, it } from "vitest";
import { runAttack } from "../src/server/runner.js";
import { PolicyEngine } from "../src/server/policy.js";
import {
  NASIKO_PROTECTED_POLICY,
  UNPROTECTED_POLICY,
} from "../src/server/policies.js";
import { getScenario } from "../src/server/scenarios.js";

const exfil = getScenario("exfiltration")!;

describe("Exfiltration scenario — unprotected vs Nasiko-protected", () => {
  it("unprotected executes the whole chain and flags exfiltration", async () => {
    const run = await runAttack(
      exfil,
      "unprotected",
      new PolicyEngine(UNPROTECTED_POLICY),
    );
    expect(run.totals.executed).toBe(exfil.steps.length);
    expect(run.totals.blocked).toBe(0);
    expect(run.threats.some((t) => t.type === "data_exfiltration")).toBe(true);
    expect(run.riskScore).toBeGreaterThanOrEqual(50);
  });

  it("protected blocks the dangerous steps (email.send auto-rejected)", async () => {
    const run = await runAttack(
      exfil,
      "protected",
      new PolicyEngine(NASIKO_PROTECTED_POLICY),
      { onApproval: async () => false }, // reject the ask
    );
    // secrets.read blocked; email.send ask→rejected; .env read is ask→rejected
    const blockedTools = run.events
      .filter((e) => e.outcome === "blocked" || e.outcome === "rejected")
      .map((e) => e.tool);
    expect(blockedTools).toContain("secrets.read");
    expect(blockedTools).toContain("email.send");
    // No exfiltration threat because nothing left the sandbox.
    expect(run.threats.some((t) => t.type === "data_exfiltration")).toBe(false);
    expect(run.riskScore).toBeLessThan(
      (
        await runAttack(
          exfil,
          "unprotected",
          new PolicyEngine(UNPROTECTED_POLICY),
        )
      ).riskScore,
    );
  });

  it("protected + human approval executes the approved ask", async () => {
    const run = await runAttack(
      exfil,
      "protected",
      new PolicyEngine(NASIKO_PROTECTED_POLICY),
      { onApproval: async () => true }, // approve
    );
    const email = run.events.find((e) => e.tool === "email.send")!;
    expect(email.outcome).toBe("approved");
  });
});

describe("Database destroyer", () => {
  it("read allowed, delete blocked under Nasiko", async () => {
    const run = await runAttack(
      getScenario("db-destroyer")!,
      "protected",
      new PolicyEngine(NASIKO_PROTECTED_POLICY),
    );
    const q = run.events.find((e) => e.tool === "database.query")!;
    const d = run.events.find((e) => e.tool === "database.delete")!;
    expect(q.outcome).toBe("executed");
    expect(d.outcome).toBe("blocked");
  });
});
