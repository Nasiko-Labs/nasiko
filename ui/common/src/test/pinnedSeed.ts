/**
 * Shared page-test setup: one fixed clock and the seed generated for it, so results never
 * depend on the run date. Call `setupPinnedSeed()` at the top of a test file.
 */
import { screen } from '@testing-library/react'
import { afterAll, afterEach, beforeAll, vi } from 'vitest'
import { configureMocks } from '@/mocks/handlers'
import { generateSeed } from '@/mocks/seed'

export const FIXED = new Date('2026-03-20T15:00:00Z')
export const seed = generateSeed({ anchor: FIXED })
export const now = () => FIXED.getTime()

/** Pin Date to FIXED and the mocks to `seed`; after each test, reset mocks, stubbed globals and spies. */
export function setupPinnedSeed(): void {
  beforeAll(() => {
    vi.useFakeTimers({ toFake: ['Date'], now: FIXED })
    configureMocks({ seed, now, loggedIn: true })
  })
  afterAll(() => vi.useRealTimers())
  afterEach(() => {
    configureMocks({ seed, now, loggedIn: true })
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
  })
}

/** The <section> a panel heading belongs to. */
export const section = async (heading: RegExp | string) =>
  (await screen.findByRole('heading', { name: heading })).closest('section')!
