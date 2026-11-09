/**
 * What connecting a coding agent sends to this server (components/ChatHistoryNotice.tsx), beside the CLI's connect
 * steps: from the server's own policy (`GET /api/me/coding-agents/privacy`), never a guess.
 */
import { screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { codingAgentsMockState, configureMocks, type MockVariant } from '@nasiko/mocks/handlers'
import { generateHarnessSeed } from '@nasiko/mocks/seed-harness'
import { FIXED, now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

const harnessSeed = generateHarnessSeed({ anchor: FIXED })

setupPinnedSeed()

function as(variant: MockVariant | null = null) {
  configureMocks({ seed, now, loggedIn: true, harnessSeed, persona: 'admin', variant })
}
afterEach(() =>
  configureMocks({ seed, now, loggedIn: true, harnessSeed, persona: null, variant: null }),
)

const card = () => screen.findByTestId('agents-onboarding')

describe('What connecting a coding agent sends', () => {
  it('on a hosted server: only usage is synced, never chat history', async () => {
    as('agents-none')
    codingAgentsMockState().chatHistory = false
    renderApp('/harnesses')
    const notice = await within(await card()).findByTestId('chat-history-notice')
    expect(notice).toHaveTextContent(copy.chatHistory.hostedTitle)
    expect(notice).toHaveTextContent('never their chat history')
    expect(notice).not.toHaveTextContent(copy.chatHistory.localTitle)
  })

  it('on a Nasiko on this computer: chat history can sync, and the CLI asks first', async () => {
    as('agents-none')
    renderApp('/harnesses')
    const notice = await within(await card()).findByTestId('chat-history-notice')
    expect(notice).toHaveTextContent(copy.chatHistory.localTitle)
    expect(notice).toHaveTextContent('asks whether to sync')
  })

  it('a server too old to say gets no notice', async () => {
    as('agents-absent')
    renderApp('/harnesses')
    await screen.findByTestId('harness-summary')
    expect(screen.queryByTestId('chat-history-notice')).toBeNull()
  })
})
