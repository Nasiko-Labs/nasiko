import { describe, expect, it } from "vitest";
import { FLAG_NAMES, parseFlagList } from "./flags";

describe("parseFlagList", () => {
  it("defaults every flag to off", () => {
    const f = parseFlagList(undefined);
    for (const n of FLAG_NAMES) expect(f[n]).toBe(false);
  });
  it("turns on only the named flags", () => {
    expect(parseFlagList("harness")).toEqual({ harness: true, weaveSurface: false });
    expect(parseFlagList(" harness , weaveSurface ")).toEqual({
      harness: true,
      weaveSurface: true,
    });
  });
  it("rejects unknown names instead of ignoring them", () => {
    expect(() => parseFlagList("harnes")).toThrow(/unknown flag/);
  });
});
