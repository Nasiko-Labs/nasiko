/** The import-free draft sweep that sign out falls back to when the chat chunk can't load (ship coverage audit). */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { clearStoredDrafts, DRAFT_PREFIX } from './draftKeys'

afterEach(() => vi.unstubAllGlobals())

describe('clearStoredDrafts', () => {
  it("removes every user's stored drafts and nothing else", () => {
    localStorage.setItem(`${DRAFT_PREFIX}u1:new:routed`, 'a')
    localStorage.setItem(`${DRAFT_PREFIX}u2:chat-1`, 'b')
    localStorage.setItem('openruntime.theme', 'dark')
    localStorage.setItem('ui-lab:other', 'kept')
    clearStoredDrafts()
    expect(localStorage.getItem(`${DRAFT_PREFIX}u1:new:routed`)).toBeNull()
    expect(localStorage.getItem(`${DRAFT_PREFIX}u2:chat-1`)).toBeNull()
    expect(localStorage.getItem('openruntime.theme')).toBe('dark')
    expect(localStorage.getItem('ui-lab:other')).toBe('kept')
  })

  it('does nothing, without throwing, when storage is blocked or missing', () => {
    const fail = () => {
      throw new Error('blocked')
    }
    vi.stubGlobal('localStorage', {
      get length(): number {
        return fail()
      },
      clear: fail,
      getItem: fail,
      key: fail,
      removeItem: fail,
      setItem: fail,
    })
    expect(() => clearStoredDrafts()).not.toThrow()
    vi.stubGlobal('localStorage', undefined)
    expect(() => clearStoredDrafts()).not.toThrow()
  })
})
