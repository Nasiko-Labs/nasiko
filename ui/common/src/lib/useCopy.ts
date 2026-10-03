import { useEffect, useRef, useState } from 'react'

/** How long an inline "Copied" or "Saved" note stays. */
export const SAVED_NOTE_MS = 3_000

type CopyState = 'idle' | 'copied' | 'failed'

/**
 * Clipboard write with a short-lived result state (failure = clipboard blocked or missing, as it is
 * outside a secure context). `run(value)` copies `value` instead, for text known only at click time.
 */
export function useCopy(text = '') {
  const [state, setState] = useState<CopyState>('idle')
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined)
  useEffect(() => () => clearTimeout(timer.current), [])
  const run = async (value = text): Promise<boolean> => {
    let ok = true
    try {
      if (!navigator.clipboard?.writeText) throw new Error('clipboard unavailable')
      await navigator.clipboard.writeText(value)
      setState('copied')
    } catch {
      ok = false
      setState('failed')
    }
    clearTimeout(timer.current)
    // Only "Copied" fades; a failure keeps the selectable text until the next try.
    if (ok) timer.current = setTimeout(() => setState('idle'), SAVED_NOTE_MS)
    return ok
  }
  return [state, run] as const
}
