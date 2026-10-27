/**
 * One-off announcements for StatusAnnouncer, the only live region in Chat (plan §8.1).
 * Components that fail outside a turn (a request answer, rename, delete, the chat list) call
 * `announce(text)` instead of adding their own role="alert".
 */
import { useStore } from 'zustand'
import { createStore } from 'zustand/vanilla'

export interface Announcement {
  text: string
  /** Bumps on every call, so the same text twice is still a new announcement. */
  seq: number
}

const store = createStore<Announcement>(() => ({ text: '', seq: 0 }))

export function announce(text: string) {
  store.setState((s) => ({ text, seq: s.seq + 1 }), true)
}

export const useAnnouncement = (): Announcement => useStore(store)
