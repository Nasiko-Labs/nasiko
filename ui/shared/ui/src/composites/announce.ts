/**
 * A page's one polite live region (DESIGN.md, the LLM router's and the Optimization page's rule): `Announcer`
 * (announcer.tsx) provides it, `useAnnounce()` speaks through it. Outside an Announcer it says nothing.
 */
import { createContext, useContext } from 'react'

export const AnnounceContext = createContext<(msg: string) => void>(() => {})
export const useAnnounce = () => useContext(AnnounceContext)
