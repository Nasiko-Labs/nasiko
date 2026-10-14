import { describe, expect, it } from "vitest";
import { redactError } from "./report-error";

describe("redactError", () => {
  it("never carries the message or stack, only digest/name/hash", () => {
    const e = Object.assign(new Error("secret-token=abc123 at /internal/path"), { digest: "d1" });
    const r = redactError(e);
    expect(JSON.stringify(r)).not.toContain("abc123");
    expect(JSON.stringify(r)).not.toContain("/internal/path");
    expect(r.digest).toBe("d1");
    expect(r.name).toBe("Error");
    expect(r.messageHash).toMatch(/^[0-9a-f]+$/);
  });
});
