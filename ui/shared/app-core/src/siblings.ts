/**
 * Previous/Next on a detail view walks the list it was opened from (docs/superpowers/plans/2026-10-06-prev-next.md).
 * The list's row links carry the ordered ids in history state, taken when the row is clicked; the detail view reads
 * them back. A deep link has no list, so it shows no arrows. History state survives a reload, not a copied link.
 */
import { useRouterState, type HistoryState } from '@tanstack/react-router'
import { useCallback, useLayoutEffect, useRef } from 'react'

declare module '@tanstack/react-router' {
  interface HistoryState {
    /** The ordered ids of the list a detail view was opened from. */
    siblings?: readonly string[]
  }
}

/**
 * A row link's `state`: one stable function for the whole list, so memoised rows don't re-render when the list
 * changes. The router calls it again when the link is clicked, so it carries the list as it is then.
 */
export function useSiblingState(ids: readonly string[]): () => HistoryState {
  const latest = useRef(ids)
  // Layout effect: a click right after a re-render must not read the previous ids.
  useLayoutEffect(() => {
    latest.current = ids
  })
  return useCallback(() => ({ siblings: latest.current }), [])
}

/** Where `id` sits in `siblings`; null without a list or when the list doesn't hold it. */
export function siblingsAround(siblings: readonly string[] | undefined, id: string) {
  const index = siblings ? siblings.indexOf(id) : -1
  if (!siblings || index < 0) return null
  return {
    siblings,
    index,
    count: siblings.length,
    prev: siblings[index - 1],
    next: siblings[index + 1],
  }
}

/** The list this view was opened from, around `id` (history state); null on a deep link. */
export function useSiblings(id: string) {
  const siblings = useRouterState({ select: (s) => s.location.state.siblings })
  return siblingsAround(siblings, id)
}
