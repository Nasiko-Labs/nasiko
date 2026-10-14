// Ported from nasiko-ui-lab@1b99bdda common/src/features/chat/keys.ts. Local edits: every key is scoped,
// `productKey(scope, "chat", …the lab's parts)`, so each builder takes the product scope first (the lab's constant
// keys `all`, `list`, `flowsAll` and `pendingAll` are builders over it); Prettier formatting.
/** Chat's query keys, in a leaf module so api.ts and sessionLookup.ts share them without a cycle. */
import type { Scope } from "@nasiko/data-access";
import { productKey, productPrefix } from "@nasiko/data-access/product";

/** The chat feature's name in its product keys. */
export const CHAT_FEATURE = "chat";

const k = (scope: Scope, ...parts: readonly unknown[]) => productKey(scope, CHAT_FEATURE, ...parts);

export const chatKeys = {
  all: (scope: Scope) => productPrefix(scope, CHAT_FEATURE),
  list: (scope: Scope) => k(scope, "list"),
  history: (scope: Scope, id: string) => k(scope, "history", id),
  flowsAll: (scope: Scope) => k(scope, "flows"),
  flows: (scope: Scope, traceId: string | null | undefined) => k(scope, "flows", traceId),
  lookup: (scope: Scope, id: string | null | undefined) => k(scope, "lookup", id),
  /** Is this Sessions id a chat of mine? (v1c §5.10) */
  probe: (scope: Scope, id: string) => k(scope, "probe", id),
  /** `GET /api/hitl/pending` for the Waiting queue (v1c §5.9), per user. */
  pendingAll: (scope: Scope) => k(scope, "pending"),
  pending: (scope: Scope, userId: string) => k(scope, "pending", userId),
};
