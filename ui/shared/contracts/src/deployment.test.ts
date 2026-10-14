import { describe, expect, it } from "vitest";
import { DeploymentModeSchema } from "./deployment";

describe("DeploymentModeSchema", () => {
  it("accepts exactly the two modes from ADR-0001", () => {
    expect(DeploymentModeSchema.parse("workspace")).toBe("workspace");
    expect(DeploymentModeSchema.parse("central")).toBe("central");
    expect(DeploymentModeSchema.safeParse("multi").success).toBe(false);
  });
});
