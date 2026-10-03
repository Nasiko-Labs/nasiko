/**
 * The router page in live mode with router, agents and providers partially mocked (VITE_NASIKO_MOCK): the live user
 * (a real `me.sub` no seed row uses) maps onto the seed admin (N19), so "Your agents" lists the seed agents, and
 * `?mock=` variants are ignored (they would mix seed and real-server states).
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { configureMocks } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { copy } from './copy'

vi.mock('@/lib/env', async (orig) => {
  const mod = await orig<typeof import('@/lib/env')>()
  return {
    ...mod,
    env: { ...mod.env, mode: 'live', partialMocks: ['router', 'agents', 'providers'] },
  }
})

setupPinnedSeed()
// These tests act on agents that follow the default: show that group (it folds by default).
beforeEach(() => localStorage.setItem('openruntime.router.defaultsOpen', 'open'))
afterEach(() => configureMocks({ seed, now, loggedIn: true, lockPersona: false }))

const REAL_SUB = '0b0b0b0b-0000-4000-8000-000000000001'

describe('partial mocks in live mode', () => {
  it('lists the seed admin’s agents for the live user and ignores ?mock=', async () => {
    configureMocks({ lockPersona: true })
    server.use(
      http.get('/api/me', () =>
        HttpResponse.json({ sub: REAL_SUB, username: 'admin', is_superuser: true }),
      ),
    )
    window.history.replaceState(null, '', '/router?mock=router-empty')
    try {
      renderApp('/router')
      await screen.findByRole('table', { name: copy.agentsTitle })
      await waitFor(() =>
        expect(
          within(screen.getByRole('navigation', { name: copy.summaryLabel })).getByRole('button', {
            name: '19 agents',
          }),
        ).toBeInTheDocument(),
      )
      expect(screen.queryByText(copy.noConfigs)).toBeNull()
      // The live user owns these rows, so the sheet offers their own configs.
      await userEvent.click(
        screen.getByRole('button', { name: `${copy.changeRouting}: Doc Writer` }),
      )
      const sheet = await screen.findByRole('dialog', { name: copy.routingTitle('Doc Writer') })
      expect(within(sheet).getByRole('radio', { name: copy.useMyDefault })).toBeChecked()
      expect(
        within(sheet).getByRole('radio', { name: new RegExp(`^${copy.useAConfig}`) }),
      ).toBeEnabled()
    } finally {
      window.history.replaceState(null, '', '/')
    }
  })
})
