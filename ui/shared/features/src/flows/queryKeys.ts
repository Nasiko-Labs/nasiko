// Ported from nasiko-ui-lab@1b99bdda common/src/features/flows/queryKeys.ts. Local edits: every key is scoped,
// `productKey(scope, "flows", …the lab's parts)`, so each builder takes the scope first (`all` and `paused` too);
// `FlowListParams` is declared here (the lab's lives in api.ts, which re-exports it), so this module imports nothing
// from the data layer; `status` and `q` admit `undefined` (exactOptionalPropertyTypes: the list page and its prefetch pass the URL's optional values); Prettier formatting.
/** Every flows query key (plans/feat-flows.md), in a module of its own so a reader of one key doesn't load the data
 * layer (the Overview's paused-flow links). */
import type { Scope } from "@nasiko/data-access";
import { productKey, productPrefix } from "@nasiko/data-access/product";

export interface FlowListParams {
  status?: string | undefined;
  q?: string | undefined;
  /** The window start, epoch ms: reading stops at the first older flow. */
  sinceMs: number;
}

/** @public The flows pages read and invalidate these. */
export const flowKeys = {
  all: (scope: Scope) => productPrefix(scope, "flows"),
  list: (scope: Scope, p: FlowListParams) =>
    productKey(scope, "flows", "list", p.status ?? "", p.q ?? "", p.sinceMs),
  detail: (scope: Scope, id: string) => productKey(scope, "flows", "detail", id),
  paused: (scope: Scope) => productKey(scope, "flows", "paused"),
};
