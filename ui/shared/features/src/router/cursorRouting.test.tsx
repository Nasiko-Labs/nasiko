/**
 * Cursor's model calls never pass through the LLM router (it reports sessions only), so its agents can't take an LLM
 * config: Change routing is off for them, says why, and the routing sheet's Previous/Next skips them. The CLI's
 * `nasiko llm-config attach` refuses them the same way (oss/cli llm_config.rs `routing_refusal`).
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { harnessIntegration } from '@nasiko/features/agents/status'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'
import { routingUnsupported } from './routing'

setupPinnedSeed()
beforeEach(() => localStorage.setItem('openruntime.router.defaultsOpen', 'open'))
afterEach(() => configureMocks({ seed, now, loggedIn: true, superuser: null, routerVariants: [] }))

const CLI = { source: 'nasiko-cli-integration' }

describe('routing for coding harnesses', () => {
  it('knows the harness behind an agent, from the detail field or the registration metadata', () => {
    expect(harnessIntegration({ coding_agent_integration_id: 'codex' })).toBe('codex')
    expect(harnessIntegration({ metadata: { ...CLI, integration_id: 'cursor' } })).toBe('cursor')
    // Not a harness: whatever its metadata says, there is no integration.
    expect(harnessIntegration({ metadata: { integration_id: 'cursor' } })).toBeNull()
    expect(harnessIntegration({})).toBeNull()
  })

  it('routes every harness but Cursor', () => {
    expect(routingUnsupported('cursor')).toBe(true)
    for (const id of ['claude', 'codex', 'opencode', null])
      expect(routingUnsupported(id)).toBe(false)
  })

  it('turns Change routing off for a Cursor agent, says why, and Previous/Next never reaches one', async () => {
    renderApp('/router')
    await screen.findByRole('table', { name: copy.agentsTitle })
    await waitFor(() => expect(screen.queryAllByText(copy.readingRouting)).toHaveLength(0))
    const buttons = screen.getAllByRole('button', { name: new RegExp(`^${copy.changeRouting}: `) })
    const cursor = buttons.filter((b) => /cursor/i.test(b.getAttribute('aria-label') ?? ''))
    expect(cursor.length).toBeGreaterThan(0)
    for (const b of cursor) {
      expect(b).toBeDisabled()
      expect(b).toHaveAccessibleDescription(copy.routingUnsupported)
    }
    // Another harness stays routable.
    const claude = buttons.find((b) => /claude/i.test(b.getAttribute('aria-label') ?? ''))
    expect(claude).toBeEnabled()
    // Stepping through the whole table from the first routable row never opens a Cursor agent.
    const routable = buttons.filter((b) => !b.hasAttribute('disabled'))
    await userEvent.click(routable[0]!)
    const sheet = screen.getByRole('dialog')
    expect(
      within(sheet).getByRole('group', { name: `agent 1 of ${routable.length}` }),
    ).toBeInTheDocument()
  })
})
