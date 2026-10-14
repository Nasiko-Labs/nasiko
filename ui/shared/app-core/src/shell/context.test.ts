// Ported from nasiko-ui-lab@1b99bdda common/src/app/shell/context.test.ts. Local edits: the TokenOps and Sessions
// page schemas the lab merges with the shared one belong to their own features (their tests), so this file checks
// the shared schema itself (`sharedSearchSchema`: junk falls back, flags parse) plus `pickShared`, `withoutWindow`,
// `compareOn` and `presenterStep`; zod/mini's `z.parse`; Prettier formatting.
/**
 * The URL contract (context.ts table): junk params fall back, `?compare=0` arrives as a
 * number, and only shared keys cross pages.
 */
import { describe, expect, it } from "vitest";
import * as z from "zod/mini";
import { compareOn, pickShared, presenterStep, sharedSearchSchema, withoutWindow } from "./context";

describe("URL contract", () => {
  it("junk falls back, flags parse, and only shared keys are carried", () => {
    const t = z.parse(sharedSearchSchema, {
      preset: "nope",
      compare: 0,
      agent: "   ",
      mock: "chaos",
      demo: 1,
      anchor: "2026-02-30",
    });
    expect(t).toMatchObject({ preset: "30d", compare: false, demo: true });
    for (const k of ["agent", "mock", "anchor"] as const) expect(t[k]).toBeUndefined();
    expect(z.parse(sharedSearchSchema, { compare: "1" }).compare).toBe(true);
    expect(z.parse(sharedSearchSchema, { preset: "7d", anchor: "2026-10-07" })).toMatchObject({
      preset: "7d",
      anchor: "2026-10-07",
    });

    expect(compareOn({})).toBe(true);
    expect(compareOn({ compare: false })).toBe(false);
    expect(
      pickShared({ preset: "7d", agent: undefined, day: "2026-03-11", open: "spend", demo: true }),
    ).toEqual({ preset: "7d", demo: true });
    expect(
      withoutWindow({ preset: "7d", from: "2026-03-01", to: "2026-03-11", agent: "a" }),
    ).toEqual({ agent: "a" });
    expect(presenterStep("/sessions/", { day: "2026-03-11" })).toBe(1);
    expect(presenterStep("/tokenops", {})).toBe(0);
    expect(presenterStep("/sessions/abc", {})).toBe(2);
    expect(presenterStep("/settings", {})).toBeNull();
  });
});
