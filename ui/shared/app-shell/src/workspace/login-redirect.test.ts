import { describe, expect, it } from "vitest";
import { workspaceLoginUrl } from "./login-redirect";

describe("workspaceLoginUrl", () => {
  it("sends a workspace page to /login and back", () => {
    expect(workspaceLoginUrl("/agents?q=a")).toBe("/login?redirect=%2Fagents%3Fq%3Da");
    expect(workspaceLoginUrl("/agents", { expired: "true" })).toBe(
      "/login?redirect=%2Fagents&expired=true",
    );
  });

  it("does nothing on /login: the workspace still painted during the move there must not redirect again", () => {
    for (const href of ["/login", "/login?redirect=%2F", "/login#x", "/login/"])
      expect(workspaceLoginUrl(href)).toBeNull();
    expect(workspaceLoginUrl("/loginsettings")).toBe("/login?redirect=%2Floginsettings");
  });

  it("never nests a foreign or protocol-relative target", () => {
    expect(workspaceLoginUrl("//evil.example/x")).toBe("/login?redirect=%2F");
  });

  it("is a fixed point: following the redirect it returns cannot produce another one", () => {
    let href = "/agents";
    const seen: string[] = [];
    for (let i = 0; i < 5; i++) {
      const next = workspaceLoginUrl(href);
      if (next === null) break;
      seen.push(next);
      href = next;
    }
    expect(seen).toEqual(["/login?redirect=%2Fagents"]);
  });
});
