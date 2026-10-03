import { act, renderHook } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { useMediaQuery } from './useMediaQuery'

describe('useMediaQuery with matchMedia', () => {
  afterEach(() => vi.unstubAllGlobals())

  it('reads the live match and re-renders on change', () => {
    let matches = false
    const listeners = new Set<() => void>()
    vi.stubGlobal('matchMedia', (query: string) => ({
      get matches() {
        return matches
      },
      media: query,
      addEventListener: (_: string, cb: () => void) => listeners.add(cb),
      removeEventListener: (_: string, cb: () => void) => listeners.delete(cb),
    }))
    const { result, unmount } = renderHook(() => useMediaQuery('(min-width: 48rem)'))
    expect(result.current).toBe(false)
    act(() => {
      matches = true
      listeners.forEach((l) => l())
    })
    expect(result.current).toBe(true)
    unmount()
    expect(listeners.size).toBe(0)
  })
})
