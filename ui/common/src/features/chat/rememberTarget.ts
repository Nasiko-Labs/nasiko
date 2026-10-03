/**
 * The last new-chat target per viewer (v1c C2), in localStorage under the drafts prefix, so every
 * sign-out path that clears drafts clears it too (`clearDrafts`, `clearStoredDrafts`). A per-viewer
 * convenience: blocked storage means nothing is remembered.
 */
import { DRAFT_PREFIX } from '@/lib/draftKeys'

export type RememberedTarget = { kind: 'orchestrator' } | { kind: 'agent'; id: string }

/** `${DRAFT_PREFIX}${sub}:meta:target` (E16); the value is `orchestrator` or an agent id. */
export const rememberedTargetKey = (sub: string) => `${DRAFT_PREFIX}${sub}:meta:target`

const ORCHESTRATOR = 'orchestrator'
const MAX_ID = 200

export function readRememberedTarget(sub: string): RememberedTarget | null {
  try {
    const v = globalThis.localStorage?.getItem(rememberedTargetKey(sub))?.trim()
    if (!v || v.length > MAX_ID) return null
    return v === ORCHESTRATOR ? { kind: 'orchestrator' } : { kind: 'agent', id: v }
  } catch {
    return null
  }
}

/** Written when the user picks a target, and after a first send; arriving with `?agent=` alone doesn't. */
export function rememberTarget(sub: string, target: RememberedTarget) {
  try {
    globalThis.localStorage?.setItem(
      rememberedTargetKey(sub),
      target.kind === 'orchestrator' ? ORCHESTRATOR : target.id,
    )
  } catch {
    // Blocked storage: nothing is remembered.
  }
}

/** The rail's selected view (v1c DX-T1): per viewer, under the drafts prefix like the target (E16). */
export type RailView = 'chats' | 'waiting' | 'recorded'
const railViewKey = (sub: string) => `${DRAFT_PREFIX}${sub}:meta:railView`

export function readRailView(sub: string): RailView | null {
  try {
    const v = globalThis.localStorage?.getItem(railViewKey(sub))
    return v === 'chats' || v === 'waiting' || v === 'recorded' ? v : null
  } catch {
    return null
  }
}

export function rememberRailView(sub: string, view: RailView) {
  try {
    globalThis.localStorage?.setItem(railViewKey(sub), view)
  } catch {
    // Blocked storage: the view resets on reload.
  }
}
