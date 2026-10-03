/** Chat's small hooks: a ticking clock and a draft mirrored to drafts.ts. */
import { useCallback, useEffect, useState } from 'react'
import { readDraft, writeDraft } from './drafts'

/**
 * The time, re-read every `intervalMs`: only a component that shows elapsed or relative time ticks. `fixed`
 * pins it (tests), and then nothing ticks.
 */
export function useNow(intervalMs: number, fixed?: number): number {
  const [now, setNow] = useState(() => Date.now())
  const ticking = fixed === undefined
  useEffect(() => {
    if (!ticking) return
    const id = setInterval(() => setNow(Date.now()), intervalMs)
    return () => clearInterval(id)
  }, [intervalMs, ticking])
  return fixed ?? now
}

/** Draft state for one chat key, mirrored to drafts.ts on every change. */
export function useDraft(userId: string, key: string) {
  const [state, setState] = useState(() => ({ key, text: readDraft(userId, key) }))
  // The key can change after mount (on a cold load `?agent=` resolves once the directory
  // loads): read that key's stored draft instead of keeping, then overwriting, the old one.
  if (state.key !== key) setState({ key, text: readDraft(userId, key) })
  const setDraft = useCallback(
    (v: string) => {
      setState({ key, text: v })
      writeDraft(userId, key, v)
    },
    [userId, key],
  )
  return [state.key === key ? state.text : readDraft(userId, key), setDraft] as const
}
