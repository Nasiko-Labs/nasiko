import { describe, expect, it } from "vitest";
import { PolicyEngine, wildcardMatch } from "../src/server/policy.js";
import {
  NASIKO_PROTECTED_POLICY,
  UNPROTECTED_POLICY,
} from "../src/server/policies.js";

describe("wildcardMatch (mirrors Nasiko glob)", () => {
  it("matches exact, prefix glob, and full wildcard, case-insensitively", () => {
    expect(wildcardMatch("secrets.read", "SECRETS.READ")).toBe(true);
    expect(wildcardMatch("gmail_*", "GMAIL_SEND_EMAIL")).toBe(true);
    expect(wildcardMatch("*", "anything.at.all")).toBe(true);
    expect(wildcardMatch("secrets.read", "database.query")).toBe(false);
  });
});

describe("PolicyEngine — priority block > ask > allow (Nasiko semantics)", () => {
  it("specific block beats broad allow", () => {
    const p = new PolicyEngine(NASIKO_PROTECTED_POLICY);
    expect(p.decide("secrets.read").access).toBe("denied");
    expect(p.decide("database.delete").access).toBe("denied");
    expect(p.decide("shell.execute").access).toBe("denied");
  });

  it("ask stance surfaces as ask, not denied or allowed", () => {
    const p = new PolicyEngine(NASIKO_PROTECTED_POLICY);
    expect(p.decide("email.send").access).toBe("ask");
    expect(p.decide("filesystem.read").access).toBe("ask");
  });

  it("allowed tools remain allowed", () => {
    const p = new PolicyEngine(NASIKO_PROTECTED_POLICY);
    expect(p.decide("web.search").access).toBe("allowed");
    expect(p.decide("database.query").access).toBe("allowed");
  });
});

describe("Unprotected policy", () => {
  it("allows everything", () => {
    const p = new PolicyEngine(UNPROTECTED_POLICY);
    for (const t of [
      "secrets.read",
      "database.delete",
      "email.send",
      "shell.execute",
    ]) {
      expect(p.decide(t).access).toBe("allowed");
    }
  });
});

describe("Default-deny connector (Nasiko `enabledConnector=false`)", () => {
  it("denies every tool regardless of allow rules", () => {
    const p = new PolicyEngine({
      label: "disabled",
      enabledConnector: false,
      rules: [{ pattern: "*", stance: "allow" }],
    });
    expect(p.decide("web.search").access).toBe("denied");
  });
});
