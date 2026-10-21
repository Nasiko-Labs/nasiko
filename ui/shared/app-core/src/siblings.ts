// Ported from the legacy ui/ on development (3aac84f66) common/src/lib/siblings.ts. Local edits: history state comes
// through the navigation port (`useHistoryState`, `HistoryState` in ./navigation) instead of TanStack's
// `useRouterState` and its `HistoryState` augmentation; Prettier formatting.
/**
 * Previous/Next on a detail view walks the list it was opened from. The list's row links carry the ordered ids in
 * history state, taken when the row is clicked; the detail view reads them back. A deep link has no list, so it shows
 * no arrows. History state survives a reload, not a copied link.
 */
import { useCallback, useLayoutEffect, useRef } from "react";
import { useHistoryState, type HistoryState } from "./navigation";

/**
 * A row link's `state`: one stable function for the whole list, so memoised rows don't re-render when the list
 * changes. The link calls it when it is clicked, so it carries the list as it is then.
 */
export function useSiblingState(ids: readonly string[]): () => HistoryState {
  const latest = useRef(ids);
  // Layout effect: a click right after a re-render must not read the previous ids.
  useLayoutEffect(() => {
    latest.current = ids;
  });
  return useCallback(() => ({ siblings: latest.current }), []);
}

export interface Siblings {
  siblings: readonly string[];
  index: number;
  count: number;
  prev: string | undefined;
  next: string | undefined;
}

/** Where `id` sits in `siblings`; null without a list or when the list doesn't hold it. */
export function siblingsAround(
  siblings: readonly string[] | undefined,
  id: string,
): Siblings | null {
  const index = siblings ? siblings.indexOf(id) : -1;
  if (!siblings || index < 0) return null;
  return {
    siblings,
    index,
    count: siblings.length,
    prev: siblings[index - 1],
    next: siblings[index + 1],
  };
}

/** The list this view was opened from, around `id` (history state); null on a deep link. */
export function useSiblings(id: string): Siblings | null {
  const { siblings } = useHistoryState();
  return siblingsAround(Array.isArray(siblings) ? siblings : undefined, id);
}

/** The state an in-page navigation on a detail view passes, so it keeps the list it was opened from. */
export function useKeepSiblings(): HistoryState | undefined {
  const { siblings } = useHistoryState();
  return siblings ? { siblings } : undefined;
}
