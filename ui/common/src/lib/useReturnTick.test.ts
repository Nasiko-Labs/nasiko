import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useReturnTick } from './useReturnTick'

function setVisibility(state: 'hidden' | 'visible') {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state })
  document.dispatchEvent(new Event('visibilitychange'))
}

describe('useReturnTick', () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['Date'], now: 0 })
  })
  afterEach(() => {
    vi.useRealTimers()
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' })
  })

  it('counts a return only once minAgeMs have passed since mount (boundary inclusive)', () => {
    const { result } = renderHook(() => useReturnTick(60_000))
    act(() => setVisibility('hidden'))
    vi.setSystemTime(59_999)
    act(() => setVisibility('visible'))
    expect(result.current).toBe(0)
    act(() => setVisibility('hidden'))
    vi.setSystemTime(60_000)
    act(() => setVisibility('visible'))
    expect(result.current).toBe(1)
  })

  it('measures the next return from the last tick, not from mount', () => {
    const { result } = renderHook(() => useReturnTick(60_000))
    vi.setSystemTime(120_000)
    act(() => setVisibility('visible'))
    expect(result.current).toBe(1)
    vi.setSystemTime(150_000)
    act(() => setVisibility('visible'))
    expect(result.current).toBe(1)
  })

  it('window focus counts as a return (visible-but-unfocused windows, second monitors)', () => {
    const { result } = renderHook(() => useReturnTick(60_000))
    vi.setSystemTime(90_000)
    act(() => {
      window.dispatchEvent(new Event('focus'))
    })
    expect(result.current).toBe(1)
  })

  it('ignores events while hidden, and stops listening after unmount', () => {
    const { result, unmount } = renderHook(() => useReturnTick(60_000))
    vi.setSystemTime(90_000)
    act(() => setVisibility('hidden'))
    expect(result.current).toBe(0)
    unmount()
    const add = vi.spyOn(document, 'addEventListener')
    act(() => setVisibility('visible'))
    expect(result.current).toBe(0)
    expect(add).not.toHaveBeenCalled()
  })
})
