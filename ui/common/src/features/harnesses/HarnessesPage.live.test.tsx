/**
 * Harnesses in live mode with harnesses partially mocked (VITE_NASIKO_MOCK): the preview badge
 * shows, and mocked sessions are never linked (their seed ids don't exist on the real Sessions page).
 */
import { screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { configureMocks } from '@/mocks/handlers'
import { generateHarnessSeed } from '@/mocks/seed-harness'
import { FIXED, now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'

vi.mock('@/lib/env', async (orig) => {
  const mod = await orig<typeof import('@/lib/env')>()
  return { ...mod, env: { ...mod.env, mode: 'live', partialMocks: ['harnesses'] } }
})

const harnessSeed = generateHarnessSeed({ anchor: FIXED })
setupPinnedSeed()

// Partial-mock live mode maps the live user onto the seed admin (N19).
const live = (lockPersona: boolean) =>
  configureMocks({
    seed,
    now,
    loggedIn: true,
    harnessSeed,
    persona: null,
    variant: null,
    lockPersona,
  })
afterEach(() => live(false))

describe('partial mocks in live mode', () => {
  it('shows the preview badge', async () => {
    live(true)
    renderApp('/harnesses')
    await screen.findByTestId('harness-summary')
    expect(screen.getByText('preview (mock)')).toBeInTheDocument()
  })

  it('does not link mocked sessions: their seed ids do not exist on the real Sessions page', async () => {
    live(true)
    renderApp('/harnesses')
    const panel = (await screen.findByRole('heading', { name: 'Recent sessions' })).closest(
      'section',
    )!
    expect(await within(panel).findAllByText(/turns ·/)).not.toHaveLength(0)
    expect(within(panel).queryAllByRole('link')).toHaveLength(0)
  })
})
