import { describe, expect, it } from "vitest";
import { NAV_ITEMS } from "@nasiko/app-core/shell/nav";
import { APP_ROUTES } from "../routes.gen";

describe("OSS SPA route inventory", () => {
  it("maps navigation into the protected workspace and preserves edition ownership", () => {
    const paths = new Map(APP_ROUTES.map((r) => [r.path, r]));
    for (const item of NAV_ITEMS) {
      expect(paths.has(item.to), item.to).toBe(true);
      expect(paths.get(item.to)?.public, item.to).not.toBe(true);
    }
    expect(paths.get("/login")?.public).toBe(true);
    expect(paths.has("/access")).toBe(false);
  });
});
