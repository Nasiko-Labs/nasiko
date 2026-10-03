/**
 * Chat v1c follow-ups (TODOS.md "Chat v1c new chat and rail follow-ups", user review 2026-09-28): the rail row keeps
 * its time whole and says every state; the new chat's description subline, "Choose another agent", the Orchestrator's
 * examples without generic prompts, recents without a repeated name, status words; more preselect rows (test 9);
 * the Waiting match source in `?debug=turn` (DX9).
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { Agent } from '@/features/agents/types'
import { configureChatMock } from '@/mocks/chatStore'
import { ADMIN_ID } from '@/mocks/seed-harness'
import { setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { copy } from './copy'
import { clearDrafts } from './drafts'
import { rowStates } from './railGroups'
import { clearChatRegistry } from './registry'
import { readRememberedTarget, rememberTarget } from './rememberTarget'
import { isGenericPrompt, orchestratorExamples } from './target'

const DONE = '5eedc000-0000-4000-8000-00000000c001'
const C7 = '5eedc000-0000-4000-8000-00000000c007'

interface MockAgent {
  id: string
  name: string
  display_name?: string | null
  description?: string | null
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
  return out.filter((a) => !a.tags.includes('coding-agent'))
}
const byName = async (name: string) => (await allAgents()).find((a) => a.name === name)!

/** Serve the directory with only the agents `keep` picks running (the rest stopped). */
async function directoryWith(
  keep: (a: MockAgent, i: number) => boolean,
  edit: (a: MockAgent) => MockAgent = (a) => a,
) {
  const served = (await allAgents()).map((a, i) =>
    edit({ ...a, status: keep(a, i) ? 'running' : 'stopped' }),
  )
  server.use(
    http.get('/api/agents', ({ request }) =>
      HttpResponse.json(Number(new URL(request.url).searchParams.get('offset') ?? 0) ? [] : served),
    ),
  )
  return served
}

setupPinnedSeed()
afterEach(() => {
  clearChatRegistry()
  clearDrafts()
  vi.unstubAllGlobals()
})

describe('the rail row', () => {
  it('keeps the time whole: only the agent name truncates', async () => {
    renderApp('/chat')
    const rail = await screen.findByRole('navigation', { name: 'Chats' })
    const row = (await within(rail).findAllByTestId('rail-row')).find((r) =>
      r.getAttribute('href')?.startsWith(`/chat/${DONE}`),
    )!
    const time = within(row).getByText(/ago$|^now$/)
    expect(time.parentElement).toHaveClass('shrink-0')
    expect(within(row).getByText('Support Bot')).toHaveClass('truncate')
  })

  it.each([
    [{ waiting: 1, live: false, unseen: false }, ['waiting for you']],
    [{ waiting: 0, live: true, unseen: false }, ['reply in progress']],
    [{ waiting: 0, live: false, unseen: true }, ['new reply']],
    [{ waiting: 2, live: true, unseen: false }, ['waiting for you', 'reply in progress']],
    [{ waiting: 1, live: false, unseen: true }, ['waiting for you', 'new reply']],
    [{ waiting: 0, live: true, unseen: true }, ['reply in progress', 'new reply']],
    [
      { waiting: 3, live: true, unseen: true },
      ['waiting for you', 'reply in progress', 'new reply'],
    ],
  ])('says every state in words (test 13): %o', (s, words) => {
    expect(rowStates({ ...s, failed: false })).toEqual(words)
  })

  it('names a failure too, after in progress and before a new reply', () => {
    expect(rowStates({ waiting: 0, live: true, failed: true, unseen: true })).toEqual([
      copy.rowInProgress,
      copy.rowFailed,
      copy.rowNewReply,
    ])
    expect(rowStates({ waiting: 0, live: false, failed: false, unseen: false })).toEqual([])
  })
})

describe("the Orchestrator's examples", () => {
  it('skip prompts that ask an agent about itself', () => {
    for (const t of [
      'What can you help me with?',
      'what do you do',
      'Hello',
      'Help',
      'How can you help?',
      'Who are you?',
    ])
      expect(isGenericPrompt(t), t).toBe(true)
    for (const t of [
      'Top 10 customers by revenue last quarter',
      'Help me write a README',
      'Helpdesk ticket 42',
    ])
      expect(isGenericPrompt(t), t).toBe(false)
  })

  it("take an agent's next example when its first is generic", () => {
    const agent = (id: string, examples: string[]) =>
      ({
        id,
        name: id,
        status: 'running',
        tags: [],
        skills: examples.map((e, i) => ({ id: `s${i}`, name: 's', examples: [e] })),
      }) as unknown as Agent
    const got = orchestratorExamples([
      agent('a', ['What can you help me with?', 'Compare three vector databases']),
      agent('b', ['Hello']),
    ])
    expect(got.map((e) => e.text)).toEqual(['Compare three vector databases'])
  })
})

describe('the new chat with a target', () => {
  it("uses the agent's description as the hero subline", async () => {
    const qa = await byName('seed-qa-tester')
    renderApp(`/chat?agent=${qa.id}`)
    const hero = await screen.findByTestId('new-chat-hero')
    await waitFor(() => expect(hero).toHaveTextContent(qa.description!))
    expect(hero).not.toHaveTextContent(copy.heroAgentSubline)
  })

  it('"Choose another agent" opens the list under the chips: the other agents and the Orchestrator', async () => {
    const qa = await byName('seed-qa-tester')
    renderApp(`/chat?agent=${qa.id}`)
    await screen.findByRole('list', { name: copy.examples })
    const toggle = screen.getByRole('button', { name: copy.chooseAnother })
    expect(toggle).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByRole('region', { name: copy.chooseTarget })).toBeNull()
    await userEvent.click(toggle)
    expect(toggle).toHaveAttribute('aria-expanded', 'true')
    const list = screen.getByRole('region', { name: copy.chooseTarget })
    const rows = within(list)
      .getAllByRole('button')
      .map((b) => b.textContent ?? '')
    expect(rows.some((t) => t.startsWith(copy.orchestratorName))).toBe(true)
    expect(rows.some((t) => t.startsWith('QA Tester'))).toBe(false)
    // Rows say the status as the Agents pages do.
    expect(rows.every((t) => !/running$/.test(t))).toBe(true)
    expect(rows.filter((t) => t.endsWith('Running')).length).toBeGreaterThan(0)
    await userEvent.click(toggle)
    expect(screen.queryByRole('region', { name: copy.chooseTarget })).toBeNull()
  })

  it('an agent with no examples shows the list at once, without itself', async () => {
    const served = await directoryWith(
      () => true,
      (a) => ({ ...a, skills: [] }),
    )
    const target = served.find((a) => a.name === 'seed-hr-helpdesk')!
    renderApp(`/chat?agent=${target.id}`)
    const list = await screen.findByRole('region', { name: copy.chooseTarget })
    const rows = within(list)
      .getAllByRole('button')
      .map((b) => b.textContent ?? '')
    expect(rows.some((t) => t.startsWith(copy.orchestratorName))).toBe(true)
    expect(rows.some((t) => t.startsWith('HR Helpdesk'))).toBe(false)
    expect(screen.queryByRole('button', { name: copy.chooseAnother })).toBeNull()
  })

  it('recents filtered to the target show only the time, not its name again', async () => {
    const bot = await byName('seed-support-bot')
    renderApp(`/chat?agent=${bot.id}`)
    const recents = await screen.findByRole('region', { name: copy.recentWithAgent })
    for (const link of within(recents).getAllByRole('link'))
      expect(link).not.toHaveTextContent('Support Bot')
  })

  it("with no target, recents still name each chat's target", async () => {
    await directoryWith(() => true)
    renderApp('/chat')
    const recents = await screen.findByRole('region', { name: copy.recentWithAgent })
    expect(
      within(recents)
        .getAllByRole('link')
        .some((l) => /Support Bot ·/.test(l.textContent ?? '')),
    ).toBe(true)
  })
})

describe('more preselect rows (test 9)', () => {
  it('no running agents: the list says so, nothing is preselected, and Send waits for a target', async () => {
    await directoryWith(() => false)
    const { router } = renderApp('/chat')
    const list = await screen.findByRole('region', { name: copy.chooseTarget })
    expect(await within(list).findByText(copy.noRunningAgents)).toBeInTheDocument()
    expect(router.state.location.search).not.toHaveProperty('agent')
    expect(screen.getByRole('button', { name: 'Send' })).toHaveAttribute('aria-disabled', 'true')
  })

  it('?auto=1 wins over a remembered agent, and nothing is written', async () => {
    const qa = await byName('seed-qa-tester')
    rememberTarget(ADMIN_ID, { kind: 'agent', id: qa.id })
    const { router } = renderApp('/chat?auto=1')
    expect(
      await screen.findByRole('heading', { level: 1, name: copy.heroOrchestratorTitle }),
    ).toBeInTheDocument()
    expect(router.state.location.search).not.toHaveProperty('agent')
    expect(readRememberedTarget(ADMIN_ID)).toEqual({ kind: 'agent', id: qa.id })
  })

  it('blocked storage: one running agent is still preselected; several are not', async () => {
    const throwing = {
      getItem: () => {
        throw new Error('blocked')
      },
      setItem: () => {
        throw new Error('blocked')
      },
      removeItem: () => {
        throw new Error('blocked')
      },
      clear: () => undefined,
      key: () => null,
      length: 0,
    }
    vi.stubGlobal('localStorage', throwing)
    const one = await directoryWith((_, i) => i === 0)
    const first = renderApp('/chat')
    await waitFor(() =>
      expect(first.router.state.location.search).toMatchObject({ agent: one[0]!.id }),
    )
    document.body.innerHTML = ''
    await directoryWith((_, i) => i < 3)
    const several = renderApp('/chat')
    expect(await screen.findByRole('region', { name: copy.chooseTarget })).toBeInTheDocument()
    expect(several.router.state.location.search).not.toHaveProperty('agent')
  })
})

describe('?debug=turn (DX9)', () => {
  it("shows how the chat's waiting requests were matched", async () => {
    configureChatMock({ waiting: true })
    renderApp(`/chat/${C7}?debug=turn`)
    const line = await screen.findByTestId('debug-waiting', {}, { timeout: 4000 })
    await waitFor(() => expect(line).toHaveTextContent(/^waiting · (chat_session_id|index) · /), {
      timeout: 4000,
    })
  })
})
