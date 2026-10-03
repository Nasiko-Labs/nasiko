/**
 * The chat chunk unavailable (offline, stale deploy; ship coverage audit, run 2): no registry was ever
 * built in this tab, so stopping chat turns (before another tab's sign-out or sign-in reloads this
 * one) still resolves, and clearLocalState still sweeps drafts an earlier page load stored.
 */
import { QueryClient } from '@tanstack/react-query'
import { describe, expect, it, vi } from 'vitest'
import { DRAFT_PREFIX } from '@/lib/draftKeys'
import { clearLocalState, stopChatTurns } from './signOut'

vi.mock('@/features/chat/registry', () => {
  throw new Error('Failed to fetch dynamically imported module')
})
vi.mock('@/features/chat/drafts', () => {
  throw new Error('Failed to fetch dynamically imported module')
})

describe('the chat chunk cannot load', () => {
  it('stopChatTurns still resolves', async () => {
    await expect(stopChatTurns()).resolves.toBeUndefined()
  })

  it('clearLocalState sweeps stored drafts and the cache when the chat chunk cannot load', async () => {
    const queryClient = new QueryClient()
    queryClient.setQueryData(['agents'], ['private row'])
    localStorage.setItem(`${DRAFT_PREFIX}u1:new:routed`, 'typed earlier')
    localStorage.setItem('openruntime.theme', 'dark')
    await clearLocalState(queryClient, 'u1')
    expect(localStorage.getItem(`${DRAFT_PREFIX}u1:new:routed`)).toBeNull()
    expect(localStorage.getItem('openruntime.theme')).toBe('dark')
    expect(queryClient.getQueryData(['agents'])).toBeUndefined()
  })
})
