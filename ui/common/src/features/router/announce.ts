import { createContext, useContext } from 'react'

/** The router page's one polite live region (`Announcer` in components/bits.tsx provides it). */
export const AnnounceContext = createContext<(msg: string) => void>(() => {})
export const useAnnounce = () => useContext(AnnounceContext)
