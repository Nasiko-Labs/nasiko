import type { Principal } from "@nasiko/contracts";

/**
 * Scoped query keys (contract §9): every key carries workspace + principal epoch, never credentials, and args in a
 * deterministic order so two equivalent arg objects share one cache entry. The product family lives in
 * product/keys.ts; the `source` family of generated
 * surfaces is defined by the Enterprise edition and only shares the scope rules below.
 */
export { canonicalArgs, type Scope } from "./canonical";
import type { Scope } from "./canonical";

export function scopeOf(p: Pick<Principal, "workspaceId" | "epoch">): Scope {
  return { workspaceId: p.workspaceId, epoch: p.epoch };
}

/** Prefix that matches every key of one scope — used to clear on workspace switch. */
export function scopePrefix(scope: Scope): readonly ["nasiko", "source", string, number] {
  return ["nasiko", "source", scope.workspaceId, scope.epoch] as const;
}

/** The key families that carry a scope at positions 2 (workspace) and 3 (epoch). */
const SCOPED_FAMILIES: ReadonlySet<unknown> = new Set(["source", "product"]);

/**
 * True when `key` belongs to a scope other than `current` (stale epoch or foreign workspace).
 * Covers both Weave `source` keys and authored-screen `product` keys (product/keys.ts), so one
 * scope switch clears and aborts both families.
 */
export function isForeignKey(key: readonly unknown[], current: Scope): boolean {
  return (
    key[0] === "nasiko" &&
    SCOPED_FAMILIES.has(key[1]) &&
    (key[2] !== current.workspaceId || key[3] !== current.epoch)
  );
}
