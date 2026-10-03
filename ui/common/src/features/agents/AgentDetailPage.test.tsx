/**
 * Agent detail (plan §10, component + mutations + security): name redirect / chooser / 404,
 * tab gating and fallback, crash card (no recorded reason; the EE layer tests its crash guardian), the non-manager view, copy fallback, the
 * error dot, roll back gating and progress, Access, Settings (the secrets note) and delete,
 * lifecycle errors and in-flight progress, and owner-controlled text rendering as text.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { fmtMoney } from '@/lib/format'
import { configureMocks } from '@/mocks/handlers'
import { ADMIN_ID } from '@/mocks/seed-harness'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequestBodies, recordRequests, server } from '@/test/setup'
import { copy } from './copy'

setupPinnedSeed()
// configureMocks keeps persona/variant unless told otherwise; the non-manager test sets both.
afterEach(() => {
  configureMocks({ seed, now, loggedIn: true, persona: null, variant: null })
  vi.unstubAllGlobals()
})

const agent = (i: number) => seed.agents[i]!
const running = agent(0)
const url = (i: number, tab?: string) => `/agents/${agent(i).id}${tab ? `?tab=${tab}` : ''}`
const title = () => screen.findByRole('heading', { level: 1 })
const tabNames = () => screen.getAllByRole('tab').map((t) => t.textContent)

describe('resolving the URL', () => {
  it('a unique name redirects to the UUID URL', async () => {
    const { router } = renderApp(`/agents/${running.name}`)
    expect(await title()).toHaveTextContent(running.display_name)
    expect(router.state.location.pathname).toBe(`/agents/${running.id}`)
  })

  it('a shared name shows a chooser', async () => {
    const twin = {
      id: '0000beef-0000-4000-8000-000000000001',
      name: running.name,
      display_name: 'Twin',
      status: 'stopped',
      owner_id: 'u2',
      tags: [],
      skills: [],
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
    }
    server.use(
      http.get('/api/agents', ({ request }) => {
        if (new URL(request.url).searchParams.get('offset') !== '0') return HttpResponse.json([])
        return HttpResponse.json([
          twin,
          { ...twin, id: running.id, display_name: running.display_name, status: 'running' },
        ])
      }),
    )
    renderApp(`/agents/${running.name}`)
    expect(await screen.findByText(`Multiple agents are named ${running.name}`)).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Twin' })).toHaveAttribute('href', `/agents/${twin.id}`)
  })

  it.each([
    ['an unknown name', '/agents/no-such-agent'],
    ['an unknown UUID', '/agents/0000dead-0000-4000-8000-000000000000'],
  ])('%s is not found', async (_, path) => {
    renderApp(path)
    expect(await screen.findByText('Agent not found or not visible.')).toBeInTheDocument()
  })
})

describe('tabs', () => {
  it('a manager sees all seven tabs', async () => {
    renderApp(url(0))
    await title()
    // Builds (plans/feat-deploy.md §6, design review 1) and MCP (plans/feat-mcp.md §5.2) joined the manage-only tabs.
    expect(tabNames()).toEqual([
      'Overview',
      'Activity',
      'Versions',
      'Builds',
      'MCP',
      'Access',
      'Settings',
    ])
  })

  it('an unknown tab falls back to Overview and drops it from the URL', async () => {
    const { router } = renderApp(url(0, 'bogus'))
    await title()
    expect(screen.getByRole('tab', { name: 'Overview' })).toHaveAttribute('aria-selected', 'true')
    await waitFor(() => expect(router.state.location.search).toEqual({}))
  })

  it('a non-manager sees three tabs and who owns the agent', async () => {
    configureMocks({ seed, now, loggedIn: true, persona: 'sam' })
    renderApp(url(0, 'settings'))
    await title()
    expect(tabNames()).toEqual(['Overview', 'Activity', 'Versions'])
    expect(
      await screen.findByText(/^Owned by .+ Only the owner or a superuser/),
    ).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Restart' })).toBeNull()
  })
})

describe('overview', () => {
  it('a crashed agent with no recorded reason falls back to recent error lines (OSS writes no crash fields)', async () => {
    renderApp(url(2))
    const card = await screen.findByRole('region', { name: 'This agent crashed' })
    expect(await within(card).findByText(copy.crashOss)).toBeInTheDocument()
    expect(within(card).queryByText(/OOMKilled/)).toBeNull()
  })

  it('OSS: a failed agent says so (not "crashed") and falls back to recent error lines', async () => {
    renderApp(url(5))
    const card = await screen.findByRole('region', { name: 'This agent failed to deploy' })
    expect(await within(card).findByText(/worker exited with code 137/)).toBeInTheDocument()
  })

  it('shows the active version and the call command', async () => {
    renderApp(url(0))
    const section = await screen.findByRole('region', { name: 'Active version' })
    expect(await within(section).findByText(/registry\.local\//)).toBeInTheDocument()
    expect(screen.getByText(`nasiko chat --agent ${running.id} "hello"`)).toBeInTheDocument()
  })

  it('a failed clipboard write leaves the text selectable and says so', async () => {
    vi.stubGlobal('navigator', {
      ...navigator,
      clipboard: { writeText: () => Promise.reject(new Error('denied')) },
    })
    renderApp(url(0))
    await title()
    await userEvent.click(screen.getAllByRole('button', { name: 'Copy ID' })[0]!)
    expect(await screen.findByRole('alert')).toHaveTextContent('Copy failed')
  })

  it('the live card is fetched only when opened', async () => {
    const reqs = recordRequests()
    try {
      const hits = () => reqs.urls.filter((u) => u.pathname.endsWith('agent-card.json'))
      renderApp(url(0))
      await title()
      expect(hits()).toHaveLength(0)
      await userEvent.click(screen.getByText('Live agent card'))
      await waitFor(() => expect(hits()).toHaveLength(1))
    } finally {
      reqs.stop()
    }
  })
})

describe('error dot on Activity', () => {
  const dot = () => screen.queryByRole('img', { name: 'Errors in the latest log lines' })

  it('shows for an agent with ERROR lines in the latest 50', async () => {
    renderApp(url(5))
    await title()
    await waitFor(() => expect(dot()).not.toBeNull())
  })

  // Server-shaped logs for one agent: 60 lines, newest first, level filtered after the limit cut.
  const logsWithError = (at: number) =>
    http.get('/api/observability/agents/:ref/logs', ({ request }) => {
      const u = new URL(request.url)
      const lines = Array.from({ length: 60 }, (_, k) => ({
        timestamp: new Date(now() - k * 60_000).toISOString(),
        level: k === at ? 'ERROR' : 'INFO',
        message: `line ${k}`,
        source: 'container',
        trace_id: null,
      }))
      const cut = lines.slice(0, Number(u.searchParams.get('limit') ?? 200))
      const level = u.searchParams.get('level')
      return HttpResponse.json(level ? cut.filter((l) => l.level === level) : cut)
    })

  it.each([
    [10, true],
    [55, false],
  ])(
    'an ERROR at line %i → dot shown: %s (the level filter runs after the 50-line cut)',
    async (at, shown) => {
      server.use(logsWithError(at))
      const reqs = recordRequests()
      try {
        renderApp(url(0))
        await title()
        await waitFor(() =>
          expect(
            reqs.urls.some(
              (u) =>
                u.searchParams.get('level') === 'ERROR' && u.searchParams.get('limit') === '50',
            ),
          ).toBe(true),
        )
        if (shown) await waitFor(() => expect(dot()).not.toBeNull())
        else {
          // Real wait, kept: an absent dot has no signal to wait on; this lets the error-log response render.
          await new Promise((r) => setTimeout(r, 50))
          expect(dot()).toBeNull()
        }
      } finally {
        reqs.stop()
      }
    },
  )
})

describe('versions and roll back', () => {
  it('only inactive, rollback-able versions get a menu; rolling back reports progress', async () => {
    renderApp(url(0, 'versions'))
    const menus = await screen.findAllByRole('button', { name: /^Actions for version / })
    // Agent 0 has two versions, 1.0.0 and the active 1.1.0: only 1.0.0 can be rolled back to.
    expect(menus.map((m) => m.getAttribute('aria-label'))).toEqual(['Actions for version 1.0.0'])
    await userEvent.click(menus[0]!)
    await userEvent.click(
      await screen.findByRole('menuitem', { name: 'Roll back to this version' }),
    )
    const dialog = await screen.findByRole('alertdialog')
    await userEvent.click(within(dialog).getByRole('button', { name: /Roll back/ }))
    expect(await screen.findByText(/Roll back queued \(build /)).toBeInTheDocument()
    // The header status follows the watch while the roll back runs.
    expect((await title()).closest('header')!.querySelector('[data-status]')).toHaveAttribute(
      'data-status',
      'deploying',
    )
  })
})

describe('access', () => {
  // OSS agent_acl rows are the agents THIS agent may call; the list used to be labelled as inbound. At ea233d20 the server
  // records it but doesn't enforce it (a2a_dispatch passes no caller to CpCallGuard), so nothing asks for confirmation
  // and the tab says it isn't enforced.
  it('OSS: the list is the agents this one may call, not enforced, with no confirmations', async () => {
    renderApp(url(0, 'access'))
    const section = await screen.findByRole('region', { name: copy.agentAcl })
    expect(await within(section).findByText(copy.aclNotEnforced)).toBeInTheDocument()
    const select = await within(section).findByRole('combobox', { name: 'Allow an agent' })
    // Radix Select: open it, wait for the directory's options, pick one.
    await userEvent.click(select)
    await waitFor(() => expect(screen.getAllByRole('option').length).toBeGreaterThan(0))
    await userEvent.click(
      screen.getByRole('option', { name: agent(1).display_name || agent(1).name }),
    )
    await userEvent.click(within(section).getByRole('button', { name: 'Allow' }))
    expect(screen.queryByRole('alertdialog')).toBeNull()
    await userEvent.click(
      await within(section).findByRole('button', { name: `Remove ${agent(1).name}` }),
    )
    expect(screen.queryByRole('alertdialog')).toBeNull()
    expect(await within(section).findByText(copy.noAcl)).toBeInTheDocument()
    // OSS says nothing about another edition's grantees (docs/lab-vs-react-migration-review.md §10.1).
    expect(screen.queryByText(/Enterprise|org unit/i)).toBeNull()
  })

  it('user search needs two characters and excludes the owner', async () => {
    renderApp(url(0, 'access'))
    const box = await screen.findByRole('textbox', { name: 'Add a user' })
    await userEvent.type(box, 'a')
    expect(screen.getByText('Type at least 2 characters.')).toBeInTheDocument()
    await userEvent.type(box, 'd')
    // "ad" matches the owner (admin) and others: the owner is never offered.
    const adds = await screen.findAllByRole('button', { name: /^Add / })
    const offered = adds.map((b) => b.closest('li')!.textContent ?? '')
    expect(offered.length).toBeGreaterThan(0)
    expect(offered.some((t) => /\badmin\b/.test(t))).toBe(false)
  })

  it('a malformed grants response shows an error, not a crash', async () => {
    server.use(http.get('/api/agents/:id/grants/users', () => HttpResponse.json({ nope: true })))
    renderApp(url(0, 'access'))
    const section = await screen.findByRole('region', { name: 'Shared with users' })
    expect(await within(section).findByText(/Something went wrong/)).toBeInTheDocument()
  })
})

describe('settings', () => {
  // Regression: ISSUE-002 (/qa 2026-09-27, .gstack/qa-reports/qa-report-localhost-2026-09-27.md): the note said
  // "Applies the next time the agent is deployed", which hid that Restart is enough (verified live: Restart
  // injects the new value, the running container doesn't see it before).
  it('the secrets note tells the owner that Restart applies a change', async () => {
    renderApp(url(1, 'settings'))
    const section = await screen.findByRole('region', { name: 'Secrets' })
    expect(
      within(section).getByText(/Restart or redeploy the agent to apply a change\./),
    ).toBeInTheDocument()
    expect(within(section).queryByText(/next time the agent is deployed/)).toBeNull()
  })

  it('leaves self-review off, but usable, when minimal-code mode goes on', async () => {
    // The agent reads an unset CODING_AGENT_SELF_REVIEW as ON, so enabling the ladder used to
    // silently start an extra model call on every edit. Enabling one feature must not enable a
    // second one you did not ask for.
    const rec = recordRequestBodies()
    renderApp(url(2, 'settings'))
    const section = await screen.findByRole('region', { name: 'Coding agent behavior' })
    const ladder = within(section).getByRole('switch', { name: 'Minimal-code mode' })
    const review = within(section).getByRole('switch', { name: 'Self-review' })
    expect(review).toBeDisabled()

    await userEvent.click(ladder)
    await waitFor(() => expect(review).toBeEnabled())
    expect(review).not.toBeChecked()

    await rec.flush()
    rec.stop()
    // Pinned off explicitly, because absent means on to the agent.
    const put = rec.requests.find((r) => r.method === 'POST' && String(r.url).includes('secrets'))
    expect(put?.body).toMatchObject({ name: 'CODING_AGENT_SELF_REVIEW', value: 'false' })
  })

  it('marks every optimisation switch as beta, with the same caveat in each place', async () => {
    // These change what a model receives and what we claim they saved. Someone deciding whether to
    // flip one deserves to know the behaviour is young — and that it is reversible. Worded once so
    // three sections cannot drift into three different degrees of caution.
    renderApp(url(1, 'settings'))
    const features = await screen.findByRole('region', { name: 'Features' })
    const tokens = screen.getByRole('region', { name: 'Token optimization' })
    for (const section of [features, tokens]) {
      const badge = within(section).getByLabelText(/^Beta\./)
      expect(badge).toHaveTextContent('Beta')
      expect(badge).toHaveAccessibleName(/safe to turn on or off at any time/)
    }
  })

  it('feature switches save at once; coding behaviour shows only for a code-work card', async () => {
    const rec = recordRequestBodies()
    renderApp(url(1, 'settings'))
    const features = await screen.findByRole('region', { name: 'Features' })
    expect(screen.getByRole('region', { name: 'Token optimization' })).toBeInTheDocument()
    expect(screen.queryByRole('region', { name: 'Coding agent behavior' })).toBeNull()
    const prompt = within(features).getByRole('switch', { name: 'Prompt comments' })
    expect(prompt).not.toBeChecked()
    await userEvent.click(prompt)
    await waitFor(() => expect(prompt).toBeChecked())
    await rec.flush()
    rec.stop()
    // The PUT replaces the metadata column, so it carries the whole bag, not just the flag.
    const put = rec.requests.find((r) => r.method === 'PUT')
    expect(put?.body).toEqual({ metadata: { features: { prompt_comments: 'enabled' } } })
  })

  it('self-review is a child of minimal-code mode: off and locked until its parent is on', async () => {
    renderApp(url(2, 'settings'))
    const section = await screen.findByRole('region', { name: 'Coding agent behavior' })
    const minimal = within(section).getByRole('switch', { name: 'Minimal-code mode' })
    const review = within(section).getByRole('switch', { name: 'Self-review' })
    expect(minimal).not.toBeChecked()
    expect(review).not.toBeChecked()
    expect(review).toBeDisabled()
    await userEvent.click(minimal)
    // Unset means on to the agent (nasiko-coding-policy `self_review_enabled`), so enabling the
    // ladder pins the secret to "false": the extra review turn is asked for, never inherited.
    await waitFor(() => expect(review).toBeEnabled())
    expect(review).not.toBeChecked()
    expect(await screen.findByText('CODING_AGENT_SELF_REVIEW')).toBeInTheDocument()
    // Removing the secret is what turns it on, so the round trip still works from here.
    await userEvent.click(review)
    await waitFor(() => expect(screen.queryByText('CODING_AGENT_SELF_REVIEW')).toBeNull())
    expect(review).toBeChecked()
    await userEvent.click(review)
    await waitFor(() => expect(review).not.toBeChecked())
    expect(await screen.findByText('CODING_AGENT_SELF_REVIEW')).toBeInTheDocument()
    await userEvent.click(minimal)
    await waitFor(() => expect(review).toBeDisabled())
    expect(review).not.toBeChecked()
  })

  it('a secret value is cleared on submit and only the name is listed', async () => {
    renderApp(url(1, 'settings'))
    const name = await screen.findByRole('textbox', { name: 'Name' })
    const value = screen.getByLabelText('Value')
    await userEvent.type(name, 'API_TOKEN')
    await userEvent.type(value, 's3cret')
    await userEvent.click(screen.getByRole('button', { name: 'Add or replace' }))
    expect(value).toHaveValue('')
    expect(await screen.findByText('API_TOKEN')).toBeInTheDocument()
    expect(screen.queryByText('s3cret')).toBeNull()
  })

  it('a dirty details form asks before a tab change drops it', async () => {
    const { router } = renderApp(url(1, 'settings'))
    await userEvent.type(await screen.findByRole('textbox', { name: 'Display name' }), ' edited')
    await userEvent.click(screen.getByRole('tab', { name: 'Overview' }))
    const dialog = await screen.findByRole('alertdialog', { name: 'Leave without saving?' })
    await userEvent.click(within(dialog).getByRole('button', { name: 'Stay' }))
    expect(router.state.location.search).toEqual({ tab: 'settings' })
    expect(screen.getByRole('textbox', { name: 'Display name' })).toHaveValue(
      `${agent(1).display_name} edited`,
    )
  })

  it('delete needs the exact name and lands on Your agents with a notice', async () => {
    const { router } = renderApp(url(1, 'settings'))
    await userEvent.click(await screen.findByRole('button', { name: 'Delete agent' }))
    const dialog = await screen.findByRole('alertdialog')
    const confirm = within(dialog).getByRole('button', { name: /Delete/ })
    expect(confirm).toBeDisabled()
    await userEvent.type(within(dialog).getByRole('textbox'), agent(1).name)
    expect(confirm).toBeEnabled()
    await userEvent.click(confirm)
    await waitFor(() => expect(router.state.location.pathname).toBe('/agents/mine'))
    // getBy inside waitFor: the page swaps its loading layout for the loaded one, re-mounting the notice.
    await waitFor(() =>
      expect(screen.getByText(`Deleted ${agent(1).display_name}.`)).toBeInTheDocument(),
    )
    expect(router.state.location.search).toEqual({})
  })

  it('runtime errors from a delete are listed in full on Your agents', async () => {
    const long = 'x'.repeat(2500)
    server.use(
      http.delete('/api/agents/:id', ({ params }) =>
        HttpResponse.json({
          deleted: true,
          agent_id: params.id,
          containers_stopped: 1,
          runtime_errors: ['container a | gone', long],
        }),
      ),
    )
    renderApp(url(1, 'settings'))
    await userEvent.click(await screen.findByRole('button', { name: 'Delete agent' }))
    const dialog = await screen.findByRole('alertdialog')
    await userEvent.type(within(dialog).getByRole('textbox'), agent(1).name)
    await userEvent.click(within(dialog).getByRole('button', { name: /Delete/ }))
    await waitFor(() =>
      expect(
        screen.getByText(/but the runtime reported errors \(1 container stopped\)/),
      ).toBeInTheDocument(),
    )
    expect(screen.getByText('container a | gone')).toBeInTheDocument()
    expect(screen.getByText(long)).toBeInTheDocument()
  })
})

describe('lifecycle errors', () => {
  it("a 403 on restart says the role can't deploy", async () => {
    server.use(
      http.post(
        '/api/containers/:id/:action',
        () => new HttpResponse('forbidden', { status: 403 }),
      ),
    )
    renderApp(url(0))
    await userEvent.click(await screen.findByRole('button', { name: 'Restart' }))
    expect(await screen.findByText("Your role can't deploy. Ask an admin.")).toBeInTheDocument()
  })

  it('a 500 on restart points at redeploying', async () => {
    server.use(
      http.post(
        '/api/containers/:id/:action',
        () => new HttpResponse('internal error', { status: 500 }),
      ),
    )
    renderApp(url(0))
    await userEvent.click(await screen.findByRole('button', { name: 'Restart' }))
    expect(await screen.findByText(/couldn't find this agent's container/)).toBeInTheDocument()
  })

  it('Stop confirms first (no call before), sends one stop, then reports Stopped', async () => {
    const reqs = recordRequests()
    try {
      const stops = () =>
        reqs.urls.filter((u) => u.pathname === `/api/containers/${running.id}/stop`)
      renderApp(url(0))
      await userEvent.click((await screen.findAllByRole('button', { name: 'Stop' }))[0]!)
      const dialog = await screen.findByRole('alertdialog')
      expect(stops()).toHaveLength(0)
      await userEvent.click(within(dialog).getByRole('button', { name: 'Stop' }))
      await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
      expect(stops()).toHaveLength(1)
      // The outcome note (role=status), not the "Stopped" badge.
      await waitFor(() =>
        expect(screen.getAllByRole('status').some((e) => e.textContent === copy.stoppedDone)).toBe(
          true,
        ),
      )
      expect(await screen.findByRole('button', { name: 'Start' })).toBeInTheDocument()
    } finally {
      reqs.stop()
    }
  })

  it('a 401 on an action sends the user to sign in', async () => {
    server.use(
      http.post(
        '/api/containers/:id/:action',
        () => new HttpResponse('unauthorized', { status: 401 }),
      ),
    )
    const { router } = renderApp(url(0))
    await userEvent.click(await screen.findByRole('button', { name: 'Restart' }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/login'))
  })

  it('a double click sends one restart', async () => {
    let calls = 0
    let answered = 0
    let release: () => void = () => {}
    const held = new Promise<void>((r) => (release = r))
    server.use(
      http.post('/api/containers/:id/:action', async () => {
        calls++
        await held
        return new HttpResponse(null, { status: 200 })
      }),
    )
    const onAnswer = ({ request }: { request: Request }) => {
      if (new URL(request.url).pathname.startsWith('/api/containers/')) answered++
    }
    server.events.on('response:mocked', onAnswer)
    try {
      renderApp(url(0))
      const b = await screen.findByRole('button', { name: 'Restart' })
      await userEvent.dblClick(b)
      await waitFor(() => expect(b).toBeDisabled())
      // Both clicks' calls (if there were two) are already held at the handler; wait until the first is answered.
      release()
      await waitFor(() => expect(answered).toBeGreaterThan(0))
      expect(calls).toBe(1)
    } finally {
      server.events.removeListener('response:mocked', onAnswer)
    }
  })

  it('a roll back to an ineligible version (400) explains itself in the dialog', async () => {
    server.use(
      http.post(
        '/api/agents/:id/rollback',
        () => new HttpResponse('version 1.0.0 is not rollback-eligible', { status: 400 }),
      ),
    )
    renderApp(url(0, 'versions'))
    await userEvent.click(
      (await screen.findAllByRole('button', { name: /^Actions for version / }))[0]!,
    )
    await userEvent.click(
      await screen.findByRole('menuitem', { name: 'Roll back to this version' }),
    )
    const dialog = await screen.findByRole('alertdialog')
    await userEvent.click(within(dialog).getByRole('button', { name: /Roll back/ }))
    expect(
      await within(dialog).findByText("This version can't be rolled back to."),
    ).toBeInTheDocument()
  })
})

// Regression: ISSUE-001 (/qa 2026-09-27, .gstack/qa-reports/qa-report-localhost-2026-09-27.md): no feedback while
// the restart call itself runs (~11 s against a real server, which redeploys before answering); "Restarting…"
// appeared only after it returned.
describe('lifecycle feedback while the call is in flight', () => {
  it.each([['restart', 'Restart', copy.restarting]] as const)(
    '%s shows its progress text before the server answers',
    async (_action, button, text) => {
      let release: () => void = () => {}
      server.use(
        http.post(
          '/api/containers/:id/:action',
          () =>
            new Promise<Response>((r) => {
              release = () => r(new HttpResponse(null, { status: 200 }))
            }),
        ),
      )
      renderApp(url(0))
      await userEvent.click(await screen.findByRole('button', { name: button }))
      expect(await screen.findByText(text)).toBeInTheDocument()
      expect(screen.getByRole('button', { name: button })).toBeDisabled()
      release()
      await waitFor(() => expect(screen.getByText(text)).toBeInTheDocument())
    },
  )

  it('start shows "Starting…" before the server answers', async () => {
    let release: () => void = () => {}
    server.use(
      http.post(
        '/api/containers/:id/:action',
        () =>
          new Promise<Response>((r) => {
            release = () => r(new HttpResponse(null, { status: 200 }))
          }),
      ),
    )
    renderApp(url(9))
    await userEvent.click(await screen.findByRole('button', { name: 'Start' }))
    expect(await screen.findByText(copy.starting)).toBeInTheDocument()
    release()
  })
})

describe('owner-controlled text', () => {
  const evil = {
    id: '0000beef-0000-4000-8000-00000000e011',
    name: 'evil',
    display_name: '<img src=x onerror=alert(1)>',
    description: '<script>alert(2)</script>',
    owner_id: ADMIN_ID,
    status: 'running',
    version: '1.0.0',
    url: '',
    iconUrl: 'javascript:alert(3)',
    documentationUrl: 'javascript:alert(4)',
    protocolVersion: '0.3.0',
    preferredTransport: 'JSONRPC',
    defaultInputModes: [],
    defaultOutputModes: [],
    capabilities: {},
    skills: [],
    tags: [],
    can_manage: true,
    is_coding_agent: false,
    created_at: '2026-03-01T00:00:00Z',
    updated_at: '2026-03-01T00:00:00Z',
  }

  it('renders as text and never links or loads a javascript: URL', async () => {
    server.use(http.get(`/api/agents/${evil.id}`, () => HttpResponse.json({ data: evil })))
    renderApp(`/agents/${evil.id}`)
    expect(await title()).toHaveTextContent('<img src=x onerror=alert(1)>')
    expect(screen.getByText('<script>alert(2)</script>')).toBeInTheDocument()
    expect(
      document.querySelector('main img[src^="javascript"], main a[href^="javascript"]'),
    ).toBeNull()
    expect(document.querySelector('main img[src="x"]')).toBeNull()
  })
})

describe('Try it (chat plan §9)', () => {
  it('a running agent links to Chat from the header and from Call this agent', async () => {
    renderApp(url(0))
    await title()
    const links = await screen.findAllByRole('link', { name: 'Try it' })
    expect(links).toHaveLength(2)
    for (const l of links) expect(l).toHaveAttribute('href', `/chat?agent=${running.id}`)
  })

  it('a harness has no Try it', async () => {
    const rows = (await (await fetch('/api/agents?limit=100&offset=0')).json()) as {
      id: string
      tags?: string[]
    }[]
    renderApp(`/agents/${rows.find((a) => a.tags?.includes('coding-agent'))!.id}`)
    await title()
    expect(screen.queryByRole('link', { name: 'Try it' })).toBeNull()
  })

  it('an agent that is not running has no Try it', async () => {
    // The header's status comes from the owner's list page (useAgentStatus).
    const detail = (await (await fetch(`/api/agents/${running.id}`)).json()) as {
      data: { owner_id: string }
    }
    const rows = (await (
      await fetch(`/api/agents?owner=${detail.data.owner_id}&limit=100`)
    ).json()) as { id: string; status: string }[]
    server.use(
      http.get('/api/agents', ({ request }) =>
        new URL(request.url).searchParams.get('owner')
          ? HttpResponse.json(
              rows.map((r) => (r.id === running.id ? { ...r, status: 'stopped' } : r)),
            )
          : undefined,
      ),
    )
    renderApp(url(0))
    await title()
    await screen.findByRole('button', { name: 'Start' })
    expect(screen.queryByRole('link', { name: 'Try it' })).toBeNull()
  })
})

describe('more states (plan §10)', () => {
  it('a coding harness shows Overview only and points at Harnesses', async () => {
    const rows = (await (await fetch('/api/agents?limit=100&offset=0')).json()) as {
      id: string
      tags?: string[]
    }[]
    const harness = rows.find((a) => a.tags?.includes('coding-agent'))!
    renderApp(`/agents/${harness.id}`)
    await title()
    expect(tabNames()).toEqual(['Overview'])
    expect(screen.getByRole('link', { name: /See usage on Harnesses/ })).toHaveAttribute(
      'href',
      expect.stringContaining('/harnesses'),
    )
    expect(screen.queryByRole('button', { name: 'Restart' })).toBeNull()
  })

  it('View raw shows the unnormalized detail as text', async () => {
    renderApp(url(0))
    await title()
    await userEvent.click(screen.getByText('View raw'))
    expect(await screen.findByText(new RegExp(`"id": "${running.id}"`))).toBeInTheDocument()
  })

  it('Settings saves the display name and says Saved', async () => {
    renderApp(url(1, 'settings'))
    const box = await screen.findByRole('textbox', { name: 'Display name' })
    await userEvent.clear(box)
    await userEvent.type(box, 'Renamed Bot')
    await userEvent.click(screen.getByRole('button', { name: 'Save' }))
    expect(await screen.findByText('Saved')).toBeInTheDocument()
    expect(await title()).toHaveTextContent('Renamed Bot')
  })

  it('the Public toggle waits for the server', async () => {
    let release: () => void = () => {}
    server.use(
      http.post(
        '/api/agents/:id/grants/public',
        () =>
          new Promise<Response>((r) => {
            release = () => r(new HttpResponse(null, { status: 200 }))
          }),
      ),
    )
    renderApp(url(1, 'access'))
    const box = await screen.findByRole('checkbox', { name: /Public/ })
    expect(box).not.toBeChecked()
    await userEvent.click(box)
    expect(box).not.toBeChecked()
    expect(box).toBeDisabled()
    release()
    await waitFor(() => expect(box).toBeEnabled())
  })

  it('a role without deployment access sees "not available", not an error', async () => {
    server.use(
      http.get('/api/agents/:id/deployment', () => HttpResponse.json({ available: false })),
    )
    renderApp(url(0))
    expect(
      await screen.findByText("Deployment details aren't available for your role."),
    ).toBeInTheDocument()
  })

  it('Activity shows the cost the server sends (recorded live stats, cost_summary.total.cost)', async () => {
    const fx = JSON.parse(
      readFileSync(join(__dirname, '../../test/__live__/oss/agents/agents.stats.json'), 'utf8'),
    ) as { body: { data: { project: { cost_summary: { total: { cost: number } } } } } }
    server.use(http.get('/api/observability/agent/:ref/stats', () => HttpResponse.json(fx.body)))
    renderApp(url(0, 'activity'))
    const cost = (await screen.findByText(copy.cost)).nextElementSibling
    expect(cost).toHaveTextContent(fmtMoney(fx.body.data.project.cost_summary.total.cost))
    expect(cost).not.toHaveTextContent('$0.00')
  })

  it('observability off (503) says how to turn it on', async () => {
    server.use(
      http.get(
        '/api/observability/agent/:ref/stats',
        () => new HttpResponse('observability not configured', { status: 503 }),
      ),
    )
    renderApp(url(0, 'activity'))
    expect(await screen.findByText(/Observability is not configured/)).toBeInTheDocument()
  })

  it('a grant to a deleted user (username null) still lists and can be removed', async () => {
    const ghost = '0000dead-0000-4000-8000-00000000beef'
    server.use(
      http.get('/api/agents/:id/grants/users', () =>
        HttpResponse.json([{ user_id: ghost, username: null }]),
      ),
    )
    renderApp(url(0, 'access'))
    expect(await screen.findByRole('button', { name: `Remove ${ghost}` })).toBeEnabled()
  })
})

describe('access on both editions (grants.ts)', () => {
  it("a /grants body in neither edition's shape fails the whole Access tab with a retry, not a crash", async () => {
    server.use(http.get('/api/agents/:id/grants', () => HttpResponse.json({ available: false })))
    renderApp(url(0, 'access'))
    for (const name of ['Shared with users', copy.agentAcl]) {
      const region = await screen.findByRole('region', { name })
      expect(await within(region).findByText(/Something went wrong/)).toBeInTheDocument()
    }
    expect(screen.queryByRole('checkbox')).toBeNull()
  })
})
