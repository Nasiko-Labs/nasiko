/**
 * Regression: ISSUE-007 — a superuser's unprovable requests for any agent held back every hub's verdict
 * Found by /qa on 2026-10-11 (PR1c)
 * Report: .gstack/qa-reports/qa-report-localhost-2026-10-11.md
 *
 * The hub took chat's `dropped` count (every agent's requests no chat proves the superuser's), so an agent with no
 * requests at all read "3 pending requests can't be tied to your chats yet" and never "No issues found". Only this
 * agent's unprovable requests count.
 */
import { screen, within } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() =>
  configureMocks({ seed, now, loggedIn: true, variant: null, persona: null, chatVariants: [] }),
)

const panel = async (name: string) =>
  (await screen.findByRole('heading', { name })).closest('section') as HTMLElement

describe('a superuser’s unprovable requests', () => {
  it('another agent’s never hold back this agent’s verdict', async () => {
    configureMocks({ chatVariants: ['waiting'] })
    const pending = (await (await fetch('/api/hitl/pending')).json()) as {
      data: { execution: { agent_id: string | null } }[]
    }
    const busy = new Set(pending.data.map((r) => r.execution.agent_id))
    const quiet = seed.agents.find((a) => !busy.has(a.id))!
    renderApp(`/agents/${quiet.id}`)
    const p = await panel(copy.needs.title)
    await within(p).findByText(/^Checked /)
    expect(within(p).queryByText(/can't be tied to your chats/)).toBeNull()
    expect(within(p).queryByText(/waiting for you$/)).toBeNull()
  })

  it('this agent’s still do', async () => {
    configureMocks({ chatVariants: ['waiting'] })
    const pending = (await (await fetch('/api/hitl/pending')).json()) as {
      data: { execution: { agent_id: string | null; chat_session_id: string | null } }[]
    }
    const target = pending.data.find((r) => !r.execution.chat_session_id)!.execution.agent_id!
    renderApp(`/agents/${target}`)
    const p = await panel(copy.needs.title)
    expect(await within(p).findByText(/can't be tied to your chats/)).toBeInTheDocument()
  })
})
