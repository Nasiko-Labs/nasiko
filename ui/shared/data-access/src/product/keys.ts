import { canonicalArgs, type Scope } from "../canonical";

/**
 * Query keys for authored (product) screens. The lab keyed its queries as
 * `['tokenops', 'dashboard', window, filters, view]`; here the same parts sit
 * behind the scope the Weave `source` keys already carry, so one workspace
 * switch (epoch.ts) clears and aborts both families:
 *
 *   ["nasiko", "product", workspaceId, epoch, feature, ...parts]
 *
 * Parts are canonicalised like source args (sorted object keys, `undefined`
 * dropped), so two equivalent filter objects share one cache entry.
 */
export type ProductKey = readonly ["nasiko", "product", string, number, string, ...unknown[]];

export function productKey(
  scope: Scope,
  feature: string,
  ...parts: readonly unknown[]
): ProductKey {
  return [
    "nasiko",
    "product",
    scope.workspaceId,
    scope.epoch,
    feature,
    ...parts.map(canonicalArgs),
  ] as const;
}

/** Prefix matching every product key of one scope, or of one feature within it (invalidation, refetch-on-return). */
export function productPrefix(
  scope: Scope,
  feature?: string,
): readonly ["nasiko", "product", string, number] | ProductKey {
  const base = ["nasiko", "product", scope.workspaceId, scope.epoch] as const;
  return feature === undefined ? base : ([...base, feature] as const);
}

/** True when `key` is a product key (any scope). */
export function isProductKey(key: readonly unknown[]): key is ProductKey {
  return (
    key[0] === "nasiko" &&
    key[1] === "product" &&
    typeof key[2] === "string" &&
    typeof key[3] === "number" &&
    typeof key[4] === "string"
  );
}
