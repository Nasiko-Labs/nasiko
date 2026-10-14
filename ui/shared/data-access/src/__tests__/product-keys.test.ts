import { describe, expect, it } from "vitest";
import { QueryClient } from "@tanstack/query-core";
import { isForeignKey } from "../keys";
import { isProductKey, productKey, productPrefix } from "../product/keys";

const A = { workspaceId: "ws-a", epoch: 0 };
const B = { workspaceId: "ws-b", epoch: 1 };

describe("productKey", () => {
  it("carries family, scope and feature, then canonicalised parts", () => {
    const k = productKey(A, "tokenops", "dashboard", { view: "agent", filters: { b: 1, a: 2 } });
    expect(k).toEqual([
      "nasiko",
      "product",
      "ws-a",
      0,
      "tokenops",
      "dashboard",
      { filters: { a: 2, b: 1 }, view: "agent" },
    ]);
    expect(isProductKey(k)).toBe(true);
    expect(isProductKey(["nasiko", "source", "ws-a", 0, "fetchUsageSummary", "1", {}])).toBe(false);
  });
  it("two equivalent part objects hash to one entry; undefined parts are dropped", () => {
    const qc = new QueryClient();
    qc.setQueryData(productKey(A, "f", { agent: undefined, model: "m" }), 1);
    expect(qc.getQueryData(productKey(A, "f", { model: "m" }))).toBe(1);
    expect(qc.getQueryData(productKey(A, "f", { model: "m", agent: "x" }))).toBeUndefined();
  });
  it("productPrefix matches the scope, or one feature in it", () => {
    const qc = new QueryClient();
    qc.setQueryData(productKey(A, "tokenops", "dashboard"), 1);
    qc.setQueryData(productKey(A, "tokenops", "timeseries"), 2);
    qc.setQueryData(productKey(A, "agents"), 3);
    qc.setQueryData(productKey(B, "tokenops", "dashboard"), 4);
    expect(qc.getQueryCache().findAll({ queryKey: productPrefix(A, "tokenops") })).toHaveLength(2);
    expect(qc.getQueryCache().findAll({ queryKey: productPrefix(A) })).toHaveLength(3);
    expect(qc.getQueryCache().findAll({ queryKey: productPrefix(B) })).toHaveLength(1);
  });
  it("never contains credentials", () => {
    expect(JSON.stringify(productKey(A, "me"))).not.toMatch(/token|cookie|bearer/i);
  });
});

describe("isForeignKey", () => {
  it("counts product keys of another workspace or epoch as foreign, like source keys", () => {
    expect(isForeignKey(productKey(A, "f"), A)).toBe(false);
    expect(isForeignKey(productKey(A, "f"), B)).toBe(true);
    expect(isForeignKey(productKey({ workspaceId: "ws-a", epoch: 1 }, "f"), A)).toBe(true);
    expect(isForeignKey(["nasiko", "source", "ws-a", 0, "fetchUsageSummary", "1", {}], B)).toBe(
      true,
    );
    // A key outside both families is never ours to drop.
    expect(isForeignKey(["something-else", "ws-z", 9], A)).toBe(false);
  });
});
