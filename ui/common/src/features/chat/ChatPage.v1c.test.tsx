/**
 * Chat v1c M0, page level (plans/feat-chat-v1c.md §7): the new chat (test 9's tables), identity
 * consistency (10), the rail and the /chat sidebar default (11), the Orchestrator's examples (17) and
 * axe on the new-chat screen, the rail and a transcript (16).
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import axe from 'axe-core'
import { http, HttpResponse } from 'msw'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { configureChatMock } from '@/mocks/chatStore'
import { ADMIN_ID } from '@/mocks/seed-harness'
import { setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { copy } from './copy'
import { clearDrafts, readDraft, writeDraft } from './drafts'
import { clearChatRegistry } from './registry'
import { readRememberedTarget, rememberTarget } from './rememberTarget'

const DONE = '5eedc000-0000-4000-8000-00000000c001'
const ROUTED = '5eedc000-0000-4000-8000-00000000c003'
const RECORDED = '5eedc000-0000-4000-8000-00000000c004'

interface MockAgent {
  id: string
  name: string
  display_name?: string | null
  status: string
  tags: string[]
  skills: unknown[]
}

async function allAgents(): Promise<MockAgent[]> {
  const out: MockAgent[] = []
  for (let offset = 0; offset < 1000; offset += 100) {
    const rows = (await (
      await fetch(new URL(`/api/agents?limit=100&offset=${offset}`, location.origin))
    ).json()) as MockAgent[]
    out.push(...rows)
    if (rows.length < 100) break
  }
  return out
}
const chattable = (a: MockAgent) => !a.tags.includes('coding-agent')
const label = (a: MockAgent) => a.display_name?.trim() || a.name

/** Serve the directory with only the agents `keep` picks running (the rest stopped). */
async function directoryWith(
  keep: (a: MockAgent, i: number) => boolean,
  edit: (a: MockAgent) => MockAgent = (a) => a,
) {
  const rows = (await allAgents()).filter(chattable)
  const served = rows.map((a, i) => edit({ ...a, status: keep(a, i) ? 'running' : 'stopped' }))
  server.use(
    http.get('/api/agents', ({ request }) =>
      HttpResponse.json(Number(new URL(request.url).searchParams.get('offset') ?? 0) ? [] : served),
    ),
  )
  return served
}

/** Every POST that would start a chat. */
function recordCreates() {
  const creates: string[] = []
  server.events.on('request:start', ({ request }) => {
    if (
      request.method === 'POST' &&
      ['/api/chat/sessions', '/api/orchestrator/a2a'].includes(new URL(request.url).pathname)
    )
      creates.push(new URL(request.url).pathname)
  })
  return creates
}

// A fixed clock: the rail's date groups (Today, Yesterday…) must not depend on the run time.
setupPinnedSeed()

const composer = () => screen.getByRole('textbox', { name: /^(Message|Ask the Orchestrator)/ })

beforeEach(() => {
  document.cookie = 'sidebar_state=; path=/; max-age=0'
})
afterEach(() => {
  server.events.removeAllListeners()
  clearChatRegistry()
  clearDrafts()
  vi.unstubAllGlobals()
  document.cookie = 'sidebar_state=; path=/; max-age=0'
})

describe('new chat: layout and flow (test 9)', () => {
  it('nothing remembered: the hero, the composer, the target list, then recents; Send waits with its reason visible', async () => {
    renderApp('/chat')
    const root = await screen.findByTestId('new-chat')
    await within(root).findByRole('region', { name: 'Choose where to send' })
    // DOM order: hero, composer (with its reason line), the target list, then recent chats.
    const order = [
      within(root).getByRole('heading', { level: 1, name: 'Start a chat' }),
      composer(),
      within(root).getByText('Choose an agent or the Orchestrator first'),
      within(root).getByRole('region', { name: 'Choose where to send' }),
      within(root).getByRole('region', { name: 'Pick up where you left off' }),
    ]
    for (let i = 1; i < order.length; i++)
      expect(
        order[i - 1]!.compareDocumentPosition(order[i]!) & Node.DOCUMENT_POSITION_FOLLOWING,
      ).toBeTruthy()
    const send = screen.getByRole('button', { name: 'Send' })
    expect(send).toHaveAttribute('aria-disabled', 'true')
    expect(composer()).toHaveAccessibleDescription(/Choose an agent or the Orchestrator first/)
  })

  it('Enter and Send with no target open the TargetPicker, keep the text and send nothing (DS3)', async () => {
    const creates = recordCreates()
    renderApp('/chat')
    await screen.findByTestId('new-chat')
    await userEvent.type(composer(), 'hello{Enter}')
    expect(await screen.findByRole('combobox', { name: 'Send to' })).toBeInTheDocument()
    await userEvent.keyboard('{Escape}')
    await waitFor(() => expect(screen.queryByRole('combobox', { name: 'Send to' })).toBeNull())
    // Esc returns focus to the chip.
    expect(screen.getByTestId('target-chip')).toHaveFocus()
    await userEvent.click(screen.getByRole('button', { name: 'Send' }))
    expect(await screen.findByRole('combobox', { name: 'Send to' })).toBeInTheDocument()
    expect(composer()).toHaveValue('hello')
    expect(creates).toEqual([])
  })

  it('picking an agent replaces the URL, keeps the text and puts focus back in the textarea', async () => {
    const { router } = renderApp('/chat')
    await screen.findByTestId('new-chat')
    await userEvent.type(composer(), 'keep this')
    const before = router.history.length
    await userEvent.click(screen.getByTestId('target-chip'))
    const option = (await screen.findAllByRole('option')).find(
      (o) => !/^Orchestrator/.test(o.textContent ?? ''),
    )!
    await userEvent.click(option)
    await waitFor(() => expect(router.state.location.search).toHaveProperty('agent'))
    expect(router.history.length).toBe(before)
    await waitFor(() => expect(composer()).toHaveFocus())
    expect(composer()).toHaveValue('keep this')
    // A user's pick is remembered (C2).
    expect(readRememberedTarget(ADMIN_ID)).toEqual({
      kind: 'agent',
      id: (router.state.location.search as { agent: string }).agent,
    })
  })

  it('a skill chip fills the composer without sending; ?auto=1 shows the Orchestrator hero', async () => {
    const creates = recordCreates()
    const agent = (await allAgents()).find(
      (a) => chattable(a) && a.status === 'running' && a.skills.length,
    )!
    const { router } = renderApp(`/chat?agent=${agent.id}`)
    expect(await screen.findByRole('heading', { level: 1, name: label(agent) })).toBeInTheDocument()
    const chips = await screen.findByRole('list', { name: 'Examples' })
    const chip = within(chips).getAllByRole('button')[0]!
    await userEvent.click(chip)
    expect(composer()).toHaveValue(chip.textContent)
    expect(creates).toEqual([])
    await router.navigate({ to: '/chat', search: { auto: 1 } as never })
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Orchestrate a task' }),
    ).toBeInTheDocument()
  })

  it('after the first send the chat opens with the composer docked, empty and focused (DS11)', async () => {
    const agent = (await allAgents()).find((a) => chattable(a) && a.status === 'running')!
    const { router } = renderApp(`/chat?agent=${agent.id}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'hello{Enter}')
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/chat\/[0-9a-f-]{36}$/))
    await waitFor(() => expect(screen.getByLabelText(/^Message /)).toHaveFocus())
    expect(screen.getByLabelText(/^Message /)).toHaveValue('')
    expect(screen.queryByTestId('new-chat')).toBeNull()
    // A successful first send remembers the target (C2).
    expect(readRememberedTarget(ADMIN_ID)).toEqual({ kind: 'agent', id: agent.id })
  })
})

describe('new chat: drafts (test 9)', () => {
  it('text follows agent → agent and agent → Orchestrator switches', async () => {
    const running = (await allAgents()).filter((a) => chattable(a) && a.status === 'running')
    renderApp(`/chat?agent=${running[0]!.id}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'move me')
    await userEvent.click(screen.getByTestId('target-chip'))
    await userEvent.click(
      await screen.findByRole('option', { name: new RegExp(`^${label(running[1]!)}`) }),
    )
    await waitFor(() =>
      expect(screen.getByTestId('target-chip')).toHaveAccessibleName(
        `Send to: ${label(running[1]!)}`,
      ),
    )
    expect(composer()).toHaveValue('move me')
    await userEvent.click(screen.getByTestId('target-chip'))
    await userEvent.click(
      (await screen.findAllByRole('option')).find((o) =>
        /^Orchestrator/.test(o.textContent ?? ''),
      )!,
    )
    expect(await screen.findByLabelText('Ask the Orchestrator')).toHaveValue('move me')
  })

  it('carrying text over a saved draft offers Undo, which brings both drafts back (DS-T1)', async () => {
    const running = (await allAgents()).filter((a) => chattable(a) && a.status === 'running')
    writeDraft(ADMIN_ID, 'new:routed', 'saved for the Orchestrator')
    renderApp(`/chat?agent=${running[0]!.id}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'new text')
    const list = screen.queryByRole('region', { name: 'Choose where to send' })
    expect(list).toBeNull()
    await userEvent.click(screen.getByTestId('target-chip'))
    await userEvent.click(
      (await screen.findAllByRole('option')).find((o) =>
        /^Orchestrator/.test(o.textContent ?? ''),
      )!,
    )
    expect(await screen.findByLabelText('Ask the Orchestrator')).toHaveValue('new text')
    const notice = await screen.findByTestId('undo-draft')
    expect(notice).toHaveTextContent('Replaced your saved draft')
    await userEvent.click(within(notice).getByRole('button', { name: 'Undo' }))
    expect(screen.getByLabelText('Ask the Orchestrator')).toHaveValue('saved for the Orchestrator')
    expect(readDraft(ADMIN_ID, `new:${running[0]!.id}`)).toBe('new text')
    expect(screen.queryByTestId('undo-draft')).toBeNull()
  })

  it('an edit ends the Undo offer', async () => {
    const running = (await allAgents()).filter((a) => chattable(a) && a.status === 'running')
    writeDraft(ADMIN_ID, 'new:routed', 'old')
    renderApp(`/chat?agent=${running[0]!.id}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'typed')
    await userEvent.click(screen.getByTestId('target-chip'))
    await userEvent.click(
      (await screen.findAllByRole('option')).find((o) =>
        /^Orchestrator/.test(o.textContent ?? ''),
      )!,
    )
    await screen.findByTestId('undo-draft')
    await userEvent.type(screen.getByLabelText('Ask the Orchestrator'), '!')
    expect(screen.queryByTestId('undo-draft')).toBeNull()
  })

  it('Back and Forward move no draft (E16)', async () => {
    const running = (await allAgents()).filter((a) => chattable(a) && a.status === 'running')
    const { router } = renderApp(`/chat?agent=${running[0]!.id}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'stays here')
    await router.navigate({ to: '/chat', search: { agent: running[1]!.id } as never })
    await waitFor(() =>
      expect(screen.getByTestId('target-chip')).toHaveAccessibleName(
        `Send to: ${label(running[1]!)}`,
      ),
    )
    expect(composer()).toHaveValue('')
    router.history.back()
    await waitFor(() =>
      expect(screen.getByTestId('target-chip')).toHaveAccessibleName(
        `Send to: ${label(running[0]!)}`,
      ),
    )
    expect(composer()).toHaveValue('stays here')
  })
})

describe('new chat: preselect (test 9, C2 and UC-A)', () => {
  it('a remembered Orchestrator is preselected even when the directory fails', async () => {
    rememberTarget(ADMIN_ID, { kind: 'orchestrator' })
    server.use(http.get('/api/agents', () => new HttpResponse('boom', { status: 500 })))
    const { router } = renderApp('/chat')
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Orchestrate a task' }),
    ).toBeInTheDocument()
    expect(router.state.location.search).toMatchObject({ auto: 1 })
  })

  it('a remembered running agent is preselected once the directory confirms it', async () => {
    const agent = (await allAgents()).filter((a) => chattable(a) && a.status === 'running')[1]!
    rememberTarget(ADMIN_ID, { kind: 'agent', id: agent.id })
    const { router } = renderApp('/chat')
    await waitFor(() => expect(router.state.location.search).toMatchObject({ agent: agent.id }))
    expect(await screen.findByRole('heading', { level: 1, name: label(agent) })).toBeInTheDocument()
  })

  it('the only running agent is preselected, and not written as the remembered target', async () => {
    const served = await directoryWith((_, i) => i === 0)
    const { router } = renderApp('/chat')
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({ agent: served[0]!.id }),
    )
    await userEvent.type(await screen.findByLabelText(/^Message /), 'x')
    expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled()
    expect(readRememberedTarget(ADMIN_ID)).toBeNull()
  })

  it('a remembered stopped agent with one other running agent preselects the running one (UC-A)', async () => {
    const served = await directoryWith((_, i) => i === 0)
    rememberTarget(ADMIN_ID, { kind: 'agent', id: served[1]!.id })
    const { router } = renderApp('/chat')
    await waitFor(() =>
      expect(router.state.location.search).toMatchObject({ agent: served[0]!.id }),
    )
  })

  it('with several running agents nothing is preselected', async () => {
    const { router } = renderApp('/chat')
    await screen.findByRole('region', { name: 'Choose where to send' })
    expect(router.state.location.search).not.toHaveProperty('agent')
    expect(router.state.location.search).not.toHaveProperty('auto')
  })

  it('text typed before the directory loads stops the preselect and stays', async () => {
    const served = (await allAgents())
      .filter(chattable)
      .map((a, i) => ({ ...a, status: i === 0 ? 'running' : 'stopped' }))
    let release!: () => void
    const gate = new Promise<void>((r) => {
      release = r
    })
    server.use(
      http.get('/api/agents', async () => {
        await gate
        return HttpResponse.json(served)
      }),
    )
    const { router } = renderApp('/chat')
    await userEvent.type(await screen.findByLabelText('Message'), 'mine')
    release()
    await screen.findByRole('region', { name: 'Choose where to send' })
    // Proves an absence (no late preselect once the directory lands).
    await new Promise((r) => setTimeout(r, 50))
    expect(router.state.location.search).not.toHaveProperty('agent')
    expect(composer()).toHaveValue('mine')
  })

  it('the Orchestrator chosen before the directory loads stays chosen', async () => {
    const served = (await allAgents())
      .filter(chattable)
      .map((a, i) => ({ ...a, status: i === 0 ? 'running' : 'stopped' }))
    let release!: () => void
    const gate = new Promise<void>((r) => {
      release = r
    })
    server.use(
      http.get('/api/agents', async () => {
        await gate
        return HttpResponse.json(served)
      }),
    )
    const { router } = renderApp('/chat')
    await screen.findByTestId('new-chat')
    await userEvent.click(screen.getByTestId('target-chip'))
    await userEvent.click(
      (await screen.findAllByRole('option')).find((o) =>
        /^Orchestrator/.test(o.textContent ?? ''),
      )!,
    )
    await waitFor(() => expect(router.state.location.search).toMatchObject({ auto: 1 }))
    release()
    // Proves an absence (the directory landing doesn't replace the choice).
    await new Promise((r) => setTimeout(r, 50))
    expect(router.state.location.search).toMatchObject({ auto: 1 })
    expect(router.state.location.search).not.toHaveProperty('agent')
  })

  it('?agent= wins over a remembered target and writes nothing', async () => {
    const running = (await allAgents()).filter((a) => chattable(a) && a.status === 'running')
    rememberTarget(ADMIN_ID, { kind: 'orchestrator' })
    const { router } = renderApp(`/chat?agent=${running[0]!.id}`)
    expect(
      await screen.findByRole('heading', { level: 1, name: label(running[0]!) }),
    ).toBeInTheDocument()
    expect(router.state.location.search).not.toHaveProperty('auto')
    expect(readRememberedTarget(ADMIN_ID)).toEqual({ kind: 'orchestrator' })
  })
})

describe('mode identity is consistent (test 10)', () => {
  it.each([
    ['an agent chat', DONE, 'agent'],
    ['an Orchestrator chat', ROUTED, 'orchestrator'],
    ['a recorded chat', RECORDED, 'recorded'],
  ] as const)(
    '%s: the rail row, the header chip and the composer target agree',
    async (_name, id, kind) => {
      renderApp(`/chat/${id}`)
      const rail = await screen.findByRole('navigation', { name: 'Chats' })
      const row = (await within(rail).findAllByTestId('rail-row')).find((r) =>
        r.getAttribute('href')?.startsWith(`/chat/${id}`),
      )!
      const chip = await screen.findByTestId('identity-chip')
      if (kind === 'agent') {
        await waitFor(() => expect(chip.textContent).not.toMatch(/^Agent/))
        const name = within(chip).getByRole('link').textContent!
        expect(row).toHaveTextContent(`${name} ·`)
        expect(await screen.findByLabelText(`Message ${name}`)).toBeInTheDocument()
      } else if (kind === 'orchestrator') {
        expect(chip).toHaveTextContent('Orchestrator')
        expect(row).toHaveTextContent('Orchestrator ·')
        expect(await screen.findByLabelText('Ask the Orchestrator')).toBeInTheDocument()
      } else {
        expect(chip).toHaveTextContent('Recorded')
        expect(row).toHaveTextContent('Recorded ·')
        expect(await screen.findByText('Recorded from a coding harness.')).toBeInTheDocument()
        expect(screen.queryByRole('textbox', { name: /^Message/ })).toBeNull()
      }
    },
  )
})

describe('the rail (test 11)', () => {
  it('Search toggles and focuses the filter, Esc closes it when empty; New chat goes to /chat', async () => {
    const { router } = renderApp(`/chat/${DONE}`)
    const rail = await screen.findByRole('navigation', { name: 'Chats' })
    const search = within(rail).getByRole('button', { name: 'Search chats' })
    expect(within(rail).queryByLabelText('Filter chats')).toBeNull()
    await userEvent.click(search)
    expect(within(rail).getByLabelText('Filter chats')).toHaveFocus()
    await userEvent.keyboard('{Escape}')
    expect(within(rail).queryByLabelText('Filter chats')).toBeNull()
    expect(search).toHaveFocus()
    await userEvent.click(within(rail).getByRole('link', { name: 'New chat' }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/chat'))
  })

  it('rows sit in date groups; the active row is marked; every row has the empty indicator slot', async () => {
    renderApp(`/chat/${DONE}`)
    const rail = await screen.findByRole('navigation', { name: 'Chats' })
    const rows = await within(rail).findAllByTestId('rail-row')
    expect(within(rail).getAllByRole('heading', { level: 3 }).length).toBeGreaterThan(1)
    expect(rows.filter((r) => r.getAttribute('aria-current') === 'page')).toHaveLength(1)
    for (const r of rows) expect(within(r).getByTestId('row-indicator')).toBeEmptyDOMElement()
    // Chats leaves recorded chats out (D1); the Recorded view lists them, naming the kind in words (DS7).
    expect(rows.some((r) => /Recorded ·/.test(r.textContent ?? ''))).toBe(false)
    await userEvent.click(within(rail).getByRole('radio', { name: 'Recorded (2)' }))
    const recorded = await within(rail).findAllByTestId('rail-row')
    expect(recorded).toHaveLength(2)
    expect(recorded.every((r) => /Recorded ·/.test(r.textContent ?? ''))).toBe(true)
  })

  it('under many-chats every group fills and Load more brings in page 2', async () => {
    configureChatMock({ manyChats: true })
    renderApp('/chat')
    const rail = await screen.findByRole('navigation', { name: 'Chats' })
    // Page 1 is 50 rows; the recorded ones sit in their own view, counted "n+" while more pages exist (DS12).
    const recordedOption = await within(rail).findByRole('radio', { name: /^Recorded \(\d+\+\)$/ })
    const recordedOnPage1 = Number(
      /\((\d+)\+\)/.exec(recordedOption.getAttribute('aria-label')!)![1],
    )
    await waitFor(() =>
      expect(within(rail).getAllByTestId('rail-row')).toHaveLength(50 - recordedOnPage1),
    )
    expect(
      within(rail)
        .getAllByRole('heading', { level: 3 })
        .map((h) => h.textContent),
    ).toEqual(['Today', 'Yesterday', 'Previous 7 days', 'Older'])
    await userEvent.click(within(rail).getByRole('button', { name: 'Load more' }))
    await waitFor(() => expect(within(rail).getAllByTestId('rail-row')).toHaveLength(59))
    expect(within(rail).getByRole('radio', { name: 'Recorded (2)' })).toBeInTheDocument()
  })

  it('with no sidebar cookie, /chat keeps the open sidebar and puts its history there, writing nothing (E1)', async () => {
    const width = window.innerWidth
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 1440 })
    try {
      const { router } = renderApp('/agents')
      const sidebar = () => document.querySelector<HTMLElement>('[data-slot="sidebar"]')
      await waitFor(() => expect(sidebar()?.getAttribute('data-state')).toBe('expanded'))
      await router.navigate({ to: '/chat' })
      // One sidebar: the history is its drill-in panel, not a second column beside it.
      const newChat = await within(sidebar()!).findByRole('link', { name: copy.newChat })
      expect(newChat).toBeInTheDocument()
      expect(sidebar()?.getAttribute('data-state')).toBe('expanded')
      expect(screen.getAllByRole('link', { name: copy.newChat })).toHaveLength(1)
      await router.navigate({ to: '/agents' })
      await waitFor(() =>
        expect(within(sidebar()!).queryByRole('link', { name: copy.newChat })).toBeNull(),
      )
      expect(document.cookie).not.toMatch(/sidebar_state=/)
    } finally {
      Object.defineProperty(window, 'innerWidth', { configurable: true, value: width })
    }
  })

  it('a sidebar cookie wins on /chat, and a user toggle writes it', async () => {
    document.cookie = 'sidebar_state=true; path=/'
    renderApp('/chat')
    const state = () => document.querySelector('[data-slot="sidebar"]')?.getAttribute('data-state')
    await waitFor(() => expect(state()).toBe('expanded'))
    fireEvent.keyDown(window, { key: 'b', metaKey: true })
    await waitFor(() => expect(state()).toBe('collapsed'))
    expect(document.cookie).toMatch(/sidebar_state=false/)
  })
})

describe("the Orchestrator's suggestions (test 17, C1)", () => {
  it('up to 3 example chips, one per running agent', async () => {
    renderApp('/chat?auto=1')
    const chips = await screen.findByRole('list', { name: 'Examples' })
    const texts = within(chips)
      .getAllByRole('button')
      .map((b) => b.textContent)
    expect(texts.length).toBeGreaterThan(0)
    expect(texts.length).toBeLessThanOrEqual(3)
    expect(new Set(texts).size).toBe(texts.length)
  })

  it('with no examples the target list shows, without the Orchestrator row', async () => {
    await directoryWith(
      () => true,
      (a) => ({ ...a, skills: [] }),
    )
    renderApp('/chat?auto=1')
    const list = await screen.findByRole('region', { name: 'Choose where to send' })
    const rows = await within(list).findAllByRole('button')
    expect(rows.length).toBeGreaterThan(0)
    expect(rows.length).toBeLessThanOrEqual(6)
    expect(rows.some((r) => /^Orchestrator/.test(r.textContent ?? ''))).toBe(false)
    expect(screen.queryByRole('list', { name: 'Examples' })).toBeNull()
  })
})

describe('axe (test 16, M0 screens)', () => {
  const check = async () => {
    const result = await axe.run(document.body, {
      rules: { 'color-contrast': { enabled: false }, region: { enabled: false } },
    })
    expect(
      result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`),
    ).toEqual([])
  }
  it('the new-chat screen and the rail', async () => {
    renderApp('/chat')
    await screen.findByRole('region', { name: 'Choose where to send' })
    await within(await screen.findByRole('navigation', { name: 'Chats' })).findAllByTestId(
      'rail-row',
    )
    await check()
  })

  it('a transcript', async () => {
    renderApp(`/chat/${DONE}`)
    await screen.findByText(/Three incidents/i)
    await check()
  })
})
