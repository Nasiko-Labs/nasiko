/** Wraps a page: one `aria-live="polite"` region, and `useAnnounce()` (announce.ts) for every outcome it reports. */
import { useCallback, useState, type ReactNode } from 'react'
import { AnnounceContext } from './announce'

export function Announcer({ children, testId }: { children: ReactNode; testId?: string }) {
  const [msg, setMsg] = useState('')
  const announce = useCallback((m: string) => {
    // Clear first so the same message twice is read twice.
    setMsg('')
    queueMicrotask(() => setMsg(m))
  }, [])
  return (
    <AnnounceContext.Provider value={announce}>
      {children}
      <p className="sr-only" aria-live="polite" role="status" data-testid={testId}>
        {msg}
      </p>
    </AnnounceContext.Provider>
  )
}
