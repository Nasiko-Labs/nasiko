/**
 * Composer drafts per `userId:chat` (plan §6.5): memory first, mirrored to localStorage
 * (try/catch, capped at DRAFT_MAX_CHARS). A new chat's draft is keyed `new:<agentId>`,
 * `new:choose` on the chooser and `new:routed` on the routed empty state (v1b §5.1).
 */
import { DRAFT_PREFIX as PREFIX } from '@/lib/draftKeys'
import { tuning } from './tuning'

const memory = new Map<string, string>()

const storageKey = (userId: string, key: string) => `${PREFIX}${userId}:${key}`

export function readDraft(userId: string, key: string): string {
  const k = storageKey(userId, key)
  const hit = memory.get(k)
  if (hit !== undefined) return hit
  try {
    return globalThis.localStorage?.getItem(k) ?? ''
  } catch {
    return ''
  }
}

export function writeDraft(userId: string, key: string, text: string) {
  const k = storageKey(userId, key)
  memory.set(k, text)
  try {
    if (!text) globalThis.localStorage?.removeItem(k)
    else if (text.length <= tuning.DRAFT_MAX_CHARS) globalThis.localStorage?.setItem(k, text)
    else globalThis.localStorage?.removeItem(k)
  } catch {
    // Storage full or blocked: the in-memory draft still works for this tab.
  }
}

/** Every draft of this user (sign out, `app/shell/signOut.ts`); all users when omitted (user unknown: another tab's sign-out, the
 *  Account unavailable row; tests). */
export function clearDrafts(userId?: string) {
  const prefix = userId ? `${PREFIX}${userId}:` : PREFIX
  if (userId) undoTokens.delete(userId)
  else undoTokens.clear()
  for (const k of [...memory.keys()]) if (k.startsWith(prefix)) memory.delete(k)
  try {
    const ls = globalThis.localStorage
    if (!ls) return
    for (let i = ls.length - 1; i >= 0; i--) {
      const k = ls.key(i)
      if (k?.startsWith(prefix)) ls.removeItem(k)
    }
  } catch {
    // Nothing to clear.
  }
}

/** A new chat's draft key: `new:routed` on the routed empty state, `new:<agentId>` for a direct one. */
export const newChatDraftKey = (
  target: { routed: true } | { agentId: string | null | undefined },
) => ('routed' in target ? 'new:routed' : `new:${target.agentId ?? ''}`)

/**
 * Move a new-chat draft between entry points (v1b §5.1, G-19): an empty source moves nothing; a
 * non-empty destination is kept and the source stays under its own key; the source is cleared
 * only once it has moved into an empty destination. Callers never move on Back/Forward.
 * Returns whether the draft moved.
 */
export function moveDraft(userId: string, from: string, to: string): boolean {
  if (from === to) return false
  const text = readDraft(userId, from)
  if (!text.trim() || readDraft(userId, to).trim()) return false
  writeDraft(userId, to, text)
  writeDraft(userId, from, '')
  return true
}

/** A displaced draft that Undo can bring back (DS-T1, E15): per user, in memory, for UNDO_DRAFT_MS. */
export interface UndoToken {
  previousKey: string
  targetKey: string
  displaced: string
  carried: string
  expiresAt: number
}

const undoTokens = new Map<string, UndoToken>()

/**
 * A user's target choice on a new chat (v1c §5.4): the text on screen follows the choice. It is
 * written to the new target's key and the old key is cleared; a draft already saved there is kept
 * for Undo (DS-T1). Nothing typed: nothing moves, and the target's saved draft loads as today.
 * Only for a user's choice (E16): never on Back/Forward, a preselect or `?agent=` resolving, which
 * keep v1b's `moveDraft` rule. Any carry ends an earlier Undo.
 */
export function carryDraft(userId: string, from: string, to: string, now = Date.now()): boolean {
  undoTokens.delete(userId)
  if (from === to) return false
  const text = readDraft(userId, from)
  if (!text.trim()) return false
  const displaced = readDraft(userId, to)
  writeDraft(userId, to, text)
  writeDraft(userId, from, '')
  if (displaced.trim() && displaced !== text)
    undoTokens.set(userId, {
      previousKey: from,
      targetKey: to,
      displaced,
      carried: text,
      expiresAt: now + tuning.UNDO_DRAFT_MS,
    })
  return true
}

/** The Undo offer for this draft key, while it lasts. */
export function pendingUndo(userId: string, key: string, now = Date.now()): UndoToken | null {
  const t = undoTokens.get(userId)
  if (!t || t.targetKey !== key) return null
  if (now >= t.expiresAt) {
    undoTokens.delete(userId)
    return null
  }
  return t
}

/** Undo a carry (E15): the displaced draft comes back on screen, the carried text goes back to its key. */
export function undoCarry(userId: string, now = Date.now()): string | null {
  const t = undoTokens.get(userId)
  undoTokens.delete(userId)
  if (!t || now >= t.expiresAt) return null
  writeDraft(userId, t.targetKey, t.displaced)
  writeDraft(userId, t.previousKey, t.carried)
  return t.displaced
}

/** Any edit or send ends the Undo offer (E15). */
export function dropUndo(userId: string) {
  undoTokens.delete(userId)
}
