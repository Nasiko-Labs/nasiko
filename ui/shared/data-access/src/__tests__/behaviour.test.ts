import { describe, expect, it } from "vitest";
import { apply, createGroupedFilters, discardDraft, editDraft, isDirty } from "../grouped-filters";
import { canonicalArgs } from "../keys";
import { describeTotal, emptyWindow, resetWindow, retain } from "../pagination";

describe("grouped filters", () => {
  it("three draft edits produce one applied version", () => {
    let s = createGroupedFilters({ range: "7d", model: "", agent: "" });
    s = editDraft(s, { range: "30d" });
    s = editDraft(s, { model: "opus" });
    s = editDraft(s, { agent: "alpha" });
    expect(s.appliedVersion).toBe(0);
    expect(isDirty(s)).toBe(true);
    s = apply(s);
    expect(s.appliedVersion).toBe(1);
    expect(s.applied).toEqual({ range: "30d", model: "opus", agent: "alpha" });
    expect(apply(s).appliedVersion).toBe(1); // idempotent when clean
  });
  it("discard returns the draft to the applied values", () => {
    const s = discardDraft(editDraft(createGroupedFilters({ q: "" }), { q: "x" }));
    expect(s.draft).toEqual({ q: "" });
  });
});

describe("keys", () => {
  it("canonicalizes arg order and drops undefined", () => {
    expect(canonicalArgs({ b: 1, a: { d: undefined, c: [3, { z: 1, y: 2 }] } })).toEqual({
      a: { c: [3, { y: 2, z: 1 }] },
      b: 1,
    });
    expect(canonicalArgs({ limit: 10, query: "a" })).toEqual(
      canonicalArgs({ query: "a", limit: 10 }),
    );
  });
});

describe("pagination window", () => {
  it("retains a bounded number of pages nearest the focus and resets on filter change", () => {
    let w = emptyWindow<number>(2);
    w = retain(w, { rows: [1], offset: 0, limit: 50, total: 2000 }, 0);
    w = retain(w, { rows: [2], offset: 50, limit: 50, total: 2000 }, 50);
    w = retain(w, { rows: [3], offset: 100, limit: 50, total: 2000 }, 100);
    expect([...w.pages.keys()].sort((a, b) => a - b)).toEqual([50, 100]);
    expect(describeTotal(2000, 100)).toBe("100 of 2000");
    expect(describeTotal("unknown", 100)).toBe("100 loaded (total unknown)");
    expect(resetWindow(w).pages.size).toBe(0);
  });
});
