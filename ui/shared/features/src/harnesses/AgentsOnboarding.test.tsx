/**
 * The coding-agent onboarding card on Harnesses (onboarding.ts, desktop.ts, components/AgentsOnboarding*.tsx).
 * Browser: the command (no consent), then the runs the server holds, with the v2 windows, sending and refused turns.
 * Desktop app: the local consent, Yes / Not now / Try again / Retry as the desktop's own POSTs, and its local run.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import {
  doneAgents,
  importingAgents,
  mockRun,
  mockWindows,
  type MockLocalRun,
  type MockSyncRun,
} from '@nasiko/mocks/codingAgents'
import {
  codingAgentsMockState,
  configureMocks,
  routerMockState,
  type MockVariant,
} from '@nasiko/mocks/handlers'
import { generateHarnessSeed } from '@nasiko/mocks/seed-harness'
import { FIXED, now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequestBodies, recordRequests } from '@nasiko/testing/setup'

const harnessSeed = generateHarnessSeed({ anchor: FIXED })
const SYNC = '/api/me/coding-agents/sync'
const DESKTOP = '/__nasiko/coding-agents'
const USAGE = '/api/observability/coding-agents/usage'
/** The mock client's device (mocks/codingAgents.ts). */
const MOCK_DEVICE = '5eed0017000000000000000000000001'

setupPinnedSeed()

function as(variant: MockVariant | null = null) {
  configureMocks({ seed, now, loggedIn: true, harnessSeed, persona: 'admin', variant })
}
afterEach(() =>
  configureMocks({ seed, now, loggedIn: true, harnessSeed, persona: null, variant: null }),
)

const card = () => screen.findByTestId('agents-onboarding')
const summary = () => screen.findByTestId('harness-summary')
const posts = (rec: ReturnType<typeof recordRequestBodies>) =>
  rec.requests.filter((r) => r.method === 'POST').map((r) => r.url.pathname)
/** A server run as the desktop holds it locally (no device of its own). */
const localOf = (run: MockSyncRun): MockLocalRun => {
  const { device_id: _d, device_name: _n, client: _c, retry_requested_at: _r, ...local } = run
  return local
}
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms))
/** An agent's row (name, then its status) by its whole text: the two sit in their own spans. */
const agentRow = (text: string) => (_: string, el: Element | null) =>
  el?.tagName === 'LI' && el.textContent === text

describe('Token optimization for your coding agents', () => {
  it("is a switch over the viewer's harnesses that writes the agent switch's field, on and back off", async () => {
    // A finished import: the viewer's harnesses came in with Token optimization off (the column default).
    as('agents-done')
    renderApp('/harnesses')
    // On the import card, where Optimize was.
    const toggle = await within(await card()).findByRole('switch', {
      name: 'On for your coding agents',
    })
    expect(toggle).not.toBeChecked()
    const rec = recordRequestBodies()
    try {
      const puts = () => rec.requests.filter((r) => r.method === 'PUT')
      await userEvent.click(toggle)
      await waitFor(() => expect(toggle).toBeChecked())
      const on = puts()
      expect(on.length).toBeGreaterThan(0)
      for (const r of on) {
        expect(r.url.pathname).toMatch(/^\/api\/agents\/[^/]+$/)
        expect(r.body).toEqual({ compress_enabled: true })
      }
      await userEvent.click(toggle)
      await waitFor(() => expect(toggle).not.toBeChecked())
      expect(
        puts()
          .slice(on.length)
          .map((r) => r.body),
      ).toEqual(on.map(() => ({ compress_enabled: false })))
    } finally {
      rec.stop()
    }
  })

  it('writes only the harnesses with an LLM config attached: the ones routed through the LLM router', async () => {
    as('agents-done')
    const attached = new Set(
      [...routerMockState().routing.entries()]
        .filter(([, r]) => r.llm_config_id)
        .map(([agent]) => agent),
    )
    renderApp('/harnesses')
    const toggle = await within(await card()).findByRole('switch', {
      name: 'On for your coding agents',
    })
    await waitFor(() => expect(toggle).toBeEnabled())
    const rec = recordRequestBodies()
    try {
      await userEvent.click(toggle)
      await waitFor(() => expect(toggle).toBeChecked())
      const written = rec.requests
        .filter((r) => r.method === 'PUT')
        .map((r) => r.url.pathname.split('/').pop()!)
      expect(written.length).toBeGreaterThan(0)
      for (const id of written) expect(attached.has(id)).toBe(true)
    } finally {
      rec.stop()
    }
  })

  it('is disabled, and says how to attach a config, while no harness has one', async () => {
    as('agents-done')
    for (const r of routerMockState().routing.values()) r.llm_config_id = null
    const rec = recordRequests()
    try {
      renderApp('/harnesses')
      const c = await card()
      const toggle = await within(c).findByRole('switch', { name: 'On for your coding agents' })
      await waitFor(() => expect(toggle).toBeDisabled())
      expect(
        within(c).getByText(/Attach an LLM config to a coding agent first/),
      ).toBeInTheDocument()
      await userEvent.click(toggle)
      expect(rec.urls.some((u) => /^\/api\/agents\/[^/]+$/.test(u.pathname))).toBe(false)
    } finally {
      rec.stop()
    }
  })
})

describe('browser: the command, no consent', () => {
  it('a viewer with no harnesses gets the command and its Copy, and nothing asks', async () => {
    as('agents-none')
    renderApp('/harnesses')
    const c = await card()
    expect(
      within(c).getByRole('heading', { name: 'Bring in your coding agents' }),
    ).toBeInTheDocument()
    expect(
      within(c).getByText('To discover your coding agents and load your previous sessions, run:'),
    ).toBeInTheDocument()
    expect(within(c).getByText('nasiko agents setup')).toBeInTheDocument()
    expect(within(c).getByRole('button', { name: 'Copy command' })).toBeInTheDocument()
    expect(within(c).queryByRole('button', { name: 'Yes, find my agents' })).toBeNull()
    await summary()
    // The card carries the same command, so the CLI panel steps aside.
    expect(screen.queryByText(/^Not connected:/)).toBeNull()
  })

  it('harnesses registered and nothing running: no card, and no polling', async () => {
    as()
    const rec = recordRequests()
    try {
      renderApp('/harnesses')
      await summary()
      await waitFor(() => expect(rec.urls.some((u) => u.pathname === SYNC)).toBe(true))
      // The desktop probe ran once and found a browser.
      expect(rec.urls.filter((u) => u.pathname === `${DESKTOP}/state`)).toHaveLength(1)
      expect(screen.queryByTestId('agents-onboarding')).toBeNull()
      const reads = rec.urls.filter((u) => u.pathname === SYNC).length
      await wait(2_500)
      expect(rec.urls.filter((u) => u.pathname === SYNC).length).toBe(reads)
    } finally {
      rec.stop()
    }
  })

  it('an older server (bare 404 on sync) and registered harnesses: no card', async () => {
    as('agents-absent')
    renderApp('/harnesses')
    await summary()
    expect(screen.queryByTestId('agents-onboarding')).toBeNull()
  })
})

describe('browser: progress from the server', () => {
  it('an importing run shows its device, the overall bar and one row per agent', async () => {
    as()
    codingAgentsMockState().runs = [mockRun('importing', importingAgents(), now())]
    renderApp('/harnesses')
    const c = await card()
    expect(
      within(c).getByRole('heading', { name: 'Bringing in your coding agents' }),
    ).toBeInTheDocument()
    const run = within(c).getByTestId('agents-onboarding-run')
    expect(run).toHaveTextContent('Example MacBook')
    expect(within(run).getByText('desktop')).toBeInTheDocument()
    expect(within(run).getByText('Found Claude Code, Codex, OpenCode')).toBeInTheDocument()
    expect(within(run).getByText('Connected')).toBeInTheDocument()
    expect(within(run).getByText(agentRow('Claude Code, 40 of 123 sessions'))).toBeInTheDocument()
    expect(within(run).getByText(agentRow('Codex, 12 sessions · 58 turns'))).toBeInTheDocument()
    expect(within(run).getByText(agentRow('OpenCode, Waiting'))).toBeInTheDocument()
    expect(within(run).getByText(agentRow('Cursor, Not on this computer'))).toBeInTheDocument()
    // (40 + 12 + 0) of (123 + 12 + 7) sessions.
    const bar = within(run).getByRole('progressbar', { name: 'Import progress on Example MacBook' })
    expect(bar).toHaveAttribute('aria-valuenow', '37')
  })

  it('a discovering run has no number yet: the bar is indeterminate', async () => {
    as()
    codingAgentsMockState().runs = [mockRun('discovering', importingAgents(), now())]
    renderApp('/harnesses')
    const bar = await within(await card()).findByRole('progressbar')
    expect(bar).not.toHaveAttribute('aria-valuenow')
  })

  it('a run that finishes while watched shows the summary, re-reads usage, and stops polling', async () => {
    as()
    const st = codingAgentsMockState()
    st.runs = [mockRun('importing', importingAgents(), now())]
    const rec = recordRequests()
    try {
      renderApp('/harnesses')
      await within(await card()).findByText(agentRow('Claude Code, 40 of 123 sessions'))
      const usageReads = () => rec.urls.filter((u) => u.pathname === USAGE).length
      const before = usageReads()
      st.runs = [mockRun('done', doneAgents(), now() + 1000)]
      const c = await card()
      expect(
        await within(c).findByRole('heading', { name: 'Your coding agents are in' }),
      ).toBeInTheDocument()
      expect(
        within(c).getByText(
          'Imported 115 sessions and 842 turns from Claude Code, Codex, OpenCode.',
        ),
      ).toBeInTheDocument()
      expect(
        within(c).getByText(agentRow("Claude Code, 98 sessions · 753 turns · 1 couldn't be read")),
      ).toBeInTheDocument()
      await waitFor(() => expect(usageReads()).toBeGreaterThan(before))
      const reads = rec.urls.filter((u) => u.pathname === SYNC).length
      await wait(2_500)
      expect(rec.urls.filter((u) => u.pathname === SYNC).length).toBe(reads)
    } finally {
      rec.stop()
    }
  })

  it('a failed run says why, with each agent error, and the command to run again', async () => {
    as('agents-none')
    const agents = importingAgents().map((a) =>
      a.id === 'claude'
        ? { ...a, import: { ...a.import, status: 'failed' as const, error: 'disk read error' } }
        : a,
    )
    codingAgentsMockState().runs = [
      mockRun('failed', agents, now(), { error: 'the server stopped answering' }),
    ]
    renderApp('/harnesses')
    const c = await card()
    expect(
      within(c).getByRole('heading', { name: "Some coding agents didn't come in" }),
    ).toBeInTheDocument()
    expect(
      within(c).getByText('The import stopped: the server stopped answering'),
    ).toBeInTheDocument()
    expect(
      within(c).getByText(agentRow('Claude Code, Import failed: disk read error')),
    ).toBeInTheDocument()
    expect(within(c).getByText('nasiko agents setup')).toBeInTheDocument()
  })

  it('the live fallback (usage endpoint absent) shows a run too', async () => {
    as('usage-404')
    codingAgentsMockState().runs = [mockRun('importing', importingAgents(), now())]
    renderApp('/harnesses')
    expect(
      await within(await card()).findByText(agentRow('Claude Code, 40 of 123 sessions')),
    ).toBeInTheDocument()
  })
})

describe('v2: history windows, sending and refused turns', () => {
  const timeline = (c: HTMLElement, device = 'Example MacBook') =>
    within(c).getByRole('list', { name: `History read on ${device}, newest first` })
  const rows = (list: HTMLElement) =>
    within(list)
      .getAllByRole('listitem')
      .map((li) => li.textContent)

  it('lists finished windows newest first, collapses the rest, and sums the remainder on one line', async () => {
    as()
    const windows = mockWindows(now(), 5)
    codingAgentsMockState().runs = [mockRun('importing', importingAgents(), now(), { windows })]
    renderApp('/harnesses')
    const c = await card()
    expect(rows(timeline(c))).toEqual([
      'Last 8 hours · 12 sessions',
      'Last 24 hours · 9 sessions',
      `${windows[2].label} · ${windows[2].sessions} sessions`,
      `Importing ${windows[5].label}…`,
      'then 24 more days and older history',
    ])
    // Progress is windows done over windows: 5 of 31.
    expect(within(c).getByRole('progressbar')).toHaveAttribute('aria-valuenow', '16')
    await userEvent.click(within(c).getByRole('button', { name: 'Show all 5 finished' }))
    expect(rows(timeline(c))).toHaveLength(7)
    expect(within(c).getByRole('button', { name: 'Show fewer' })).toHaveAttribute(
      'aria-expanded',
      'true',
    )
    expect(within(c).queryByText(/Your last month is in/)).toBeNull()
  })

  it('says the last month is in while only "Older" is still loading', async () => {
    as()
    codingAgentsMockState().runs = [
      mockRun('importing', importingAgents(), now(), { windows: mockWindows(now(), 30) }),
    ]
    renderApp('/harnesses')
    const c = await card()
    expect(
      within(c).getByText('Your last month is in. Older history is still loading.'),
    ).toBeInTheDocument()
    expect(within(timeline(c)).getByText('Importing Older…')).toBeInTheDocument()
  })

  it('a sending run says what is still queued and never reads as done', async () => {
    as('agents-sending')
    renderApp('/harnesses')
    const c = await card()
    expect(
      within(c).getByRole('heading', { name: 'Bringing in your coding agents' }),
    ).toBeInTheDocument()
    expect(
      within(c).getByText(/^Waiting to send \d+ turns · retrying automatically$/),
    ).toBeInTheDocument()
    expect(within(c).queryByRole('heading', { name: 'Your coding agents are in' })).toBeNull()
    expect(within(c).queryByText(/^Imported /)).toBeNull()
    expect(Number(within(c).getByRole('progressbar').getAttribute('aria-valuenow'))).toBeLessThan(
      100,
    )
  })

  it('a done run with turns still queued stays a sending run', async () => {
    as()
    const agents = doneAgents().map((a) =>
      a.id === 'claude' ? { ...a, import: { ...a.import, pending: 4 } } : a,
    )
    codingAgentsMockState().runs = [mockRun('done', agents, now())]
    renderApp('/harnesses')
    const c = await card()
    expect(
      within(c).getByText('Waiting to send 4 turns · retrying automatically'),
    ).toBeInTheDocument()
    expect(within(c).queryByRole('heading', { name: 'Your coding agents are in' })).toBeNull()
  })

  it('refused turns list their reasons; Retry asks the server for that device and confirms', async () => {
    as('agents-rejected')
    const rec = recordRequestBodies()
    try {
      renderApp('/harnesses')
      const c = await card()
      expect(
        within(c).getByRole('heading', { name: "Some turns couldn't be sent" }),
      ).toBeInTheDocument()
      expect(
        within(c).getByText(
          "OpenCode: 3 turns couldn't be sent: turn larger than 1 MB (2); missing timestamp (1)",
        ),
      ).toBeInTheDocument()
      await userEvent.click(within(c).getByRole('button', { name: 'Retry' }))
      expect(
        await within(c).findByText('Retry requested — your computer will resend them shortly'),
      ).toBeInTheDocument()
      await rec.flush()
      expect(posts(rec)).toEqual([`${SYNC}/${MOCK_DEVICE}/retry`])
      expect(within(c).queryByRole('button', { name: 'Retry' })).toBeNull()
    } finally {
      rec.stop()
    }
  })

  it('a v1 run (no windows, pending or refused turns) renders with the session math', async () => {
    as()
    const {
      windows: _w,
      retry_requested_at: _r,
      ...v1
    } = mockRun('importing', importingAgents(), now())
    const v1Agents = v1.agents.map(({ import: imp, ...a }) => {
      const { pending: _p, rejected: _x, rejected_reasons: _rr, ...old } = imp
      return { ...a, import: old }
    })
    codingAgentsMockState().runs = [{ ...v1, agents: v1Agents } as unknown as MockSyncRun]
    renderApp('/harnesses')
    const c = await card()
    expect(within(c).getByText(agentRow('Claude Code, 40 of 123 sessions'))).toBeInTheDocument()
    expect(within(c).getByRole('progressbar')).toHaveAttribute('aria-valuenow', '37')
    expect(within(c).queryByRole('list', { name: /History read on/ })).toBeNull()
    expect(within(c).queryByText(/Waiting to send/)).toBeNull()
  })
})

describe('desktop app: local consent and progress', () => {
  it('asks; Yes POSTs /start and the progress shows at once, without the server', async () => {
    as('desktop-unasked')
    const rec = recordRequestBodies()
    try {
      renderApp('/harnesses')
      const c = await card()
      expect(
        within(c).getByRole('heading', { name: 'Bring in your coding agents?' }),
      ).toBeInTheDocument()
      await userEvent.click(within(c).getByRole('button', { name: 'Yes, find my agents' }))
      expect(
        await within(c).findByRole('heading', { name: 'Bringing in your coding agents' }),
      ).toBeInTheDocument()
      expect(within(c).getByTestId('agents-onboarding-run')).toHaveTextContent('This computer')
      await rec.flush()
      expect(posts(rec)).toEqual([`${DESKTOP}/start`])
      expect(rec.requests.some((r) => r.url.pathname === SYNC)).toBe(false)
      expect(codingAgentsMockState().desktop?.consent).toBe('granted')
    } finally {
      rec.stop()
    }
  })

  it('a start that fails says so, and Try again POSTs /start again', async () => {
    as('desktop-unasked')
    codingAgentsMockState().desktop!.failStarts = 1
    const rec = recordRequestBodies()
    try {
      renderApp('/harnesses')
      const c = await card()
      await userEvent.click(within(c).getByRole('button', { name: 'Yes, find my agents' }))
      const alert = await within(c).findByRole('alert')
      expect(alert).toHaveTextContent(
        "Couldn't reach the Nasiko desktop app: the desktop app could not start the import",
      )
      await userEvent.click(within(alert).getByRole('button', { name: 'Try again' }))
      expect(
        await within(c).findByRole('heading', { name: 'Bringing in your coding agents' }),
      ).toBeInTheDocument()
      await rec.flush()
      expect(posts(rec)).toEqual([`${DESKTOP}/start`, `${DESKTOP}/start`])
    } finally {
      rec.stop()
    }
  })

  it('Not now POSTs /decline, which the desktop keeps; Find my agents starts after all', async () => {
    as('desktop-unasked')
    const rec = recordRequestBodies()
    try {
      renderApp('/harnesses')
      const c = await card()
      await userEvent.click(within(c).getByRole('button', { name: 'Not now' }))
      expect(
        await within(c).findByText('You chose not to bring in your coding agents.'),
      ).toBeInTheDocument()
      expect(codingAgentsMockState().desktop?.consent).toBe('declined')
      await userEvent.click(within(c).getByRole('button', { name: 'Find my agents' }))
      expect(
        await within(c).findByRole('heading', { name: 'Bringing in your coding agents' }),
      ).toBeInTheDocument()
      await rec.flush()
      expect(posts(rec)).toEqual([`${DESKTOP}/decline`, `${DESKTOP}/start`])
    } finally {
      rec.stop()
    }
  })

  it('a declined desktop shows its quiet line on the next visit', async () => {
    as('desktop-unasked')
    codingAgentsMockState().desktop!.consent = 'declined'
    renderApp('/harnesses')
    expect(
      await within(await card()).findByText('You chose not to bring in your coding agents.'),
    ).toBeInTheDocument()
  })

  it('a failed run says why, and Try again POSTs /start', async () => {
    as('desktop-failed')
    const rec = recordRequestBodies()
    try {
      renderApp('/harnesses')
      const c = await card()
      expect(within(c).getByText('The import stopped: signed out of Nasiko')).toBeInTheDocument()
      await userEvent.click(within(c).getByRole('button', { name: 'Try again' }))
      expect(
        await within(c).findByRole('heading', { name: 'Bringing in your coding agents' }),
      ).toBeInTheDocument()
      await rec.flush()
      expect(posts(rec)).toEqual([`${DESKTOP}/start`])
    } finally {
      rec.stop()
    }
  })

  it('refused turns: Retry POSTs the desktop /retry and confirms', async () => {
    as('desktop-rejected')
    const rec = recordRequestBodies()
    try {
      renderApp('/harnesses')
      const c = await card()
      expect(
        within(c).getByText(
          "OpenCode: 3 turns couldn't be sent: turn larger than 1 MB (2); missing timestamp (1)",
        ),
      ).toBeInTheDocument()
      await userEvent.click(within(c).getByRole('button', { name: 'Retry' }))
      expect(
        await within(c).findByText('Retry requested — your computer will resend them shortly'),
      ).toBeInTheDocument()
      await rec.flush()
      expect(posts(rec)).toEqual([`${DESKTOP}/retry`])
    } finally {
      rec.stop()
    }
  })

  it('re-reads the local state while a run is active, and stops once it is done', async () => {
    as('desktop-running')
    const rec = recordRequests()
    try {
      renderApp('/harnesses')
      await card()
      const reads = () => rec.urls.filter((u) => u.pathname === `${DESKTOP}/state`).length
      await waitFor(() => expect(reads()).toBeGreaterThan(1), { timeout: 3_000 })
      codingAgentsMockState().desktop!.run = localOf(
        mockRun('done', doneAgents(), now(), { windows: mockWindows(now(), 31) }),
      )
      expect(
        await within(await card()).findByRole('heading', { name: 'Your coding agents are in' }),
      ).toBeInTheDocument()
      const settled = reads()
      await wait(2_000)
      expect(reads()).toBe(settled)
    } finally {
      rec.stop()
    }
  })
})
