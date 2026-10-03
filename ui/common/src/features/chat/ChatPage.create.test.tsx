/**
 * A new direct chat: the placeholder row and the stay-put guards while the chat is being created
 * (EN8), and the draft through a cold load, an example chip and a failed create's Try again.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { ADMIN_ID } from '@/mocks/seed-harness'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { clearDrafts, readDraft, writeDraft } from './drafts'
import { clearChatRegistry } from './registry'

const DONE = '5eedc000-0000-4000-8000-00000000c001'

afterEach(() => {
  clearChatRegistry()
  clearDrafts()
})

async function runningAgent(): Promise<{ id: string; name: string }> {
  for (let offset = 0; offset < 1000; offset += 100) {
    const rows = (await (
      await fetch(new URL(`/api/agents?limit=100&offset=${offset}`, location.origin))
    ).json()) as { id: string; name: string; status: string; tags: string[] }[]
    const hit = rows.find((a) => a.status === 'running' && !a.tags.includes('coding-agent'))
    if (hit) return hit
    if (rows.length < 100) break
  }
  throw new Error('no running agent in the mock seed')
}

/** Hold POST /api/chat/sessions until `release()`; the real handler answers afterwards. */
function slowCreate() {
  let release!: () => void
  const gate = new Promise<void>((r) => {
    release = r
  })
  server.use(
    http.post('/api/chat/sessions', async () => {
      await gate
      return undefined
    }),
  )
  return () => release()
}

/** Resolves once the user row's POST starts: it follows the create (and any redirect it would fire). */
function userRowSaved() {
  let saved = false
  const onStart = ({ request }: { request: Request }) => {
    if (request.method === 'POST' && /\/messages$/.test(new URL(request.url).pathname)) saved = true
  }
  server.events.on('request:start', onStart)
  return async () => {
    await waitFor(() => expect(saved).toBe(true))
    server.events.removeListener('request:start', onStart)
  }
}

describe('new chat: while the chat is being created', () => {
  it('opening the placeholder row while the chat is being created is not "deleted or not yours"', async () => {
    const release = slowCreate()
    const agent = await runningAgent()
    renderApp(`/chat?agent=${agent.id}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'still creating{Enter}')
    const rail = screen.getByRole('navigation', { name: 'Chats' })
    await userEvent.click(await within(rail).findByRole('link', { name: /still creating/ }))
    // Proves an absence (no "deleted" state while the create is held): give the page a moment.
    await new Promise((r) => setTimeout(r, 50))
    expect(screen.queryByText("This chat was deleted or isn't yours.")).toBeNull()
    release()
    await waitFor(() =>
      expect(
        within(screen.getByTestId('transcript')).getByText('still creating'),
      ).toBeInTheDocument(),
    )
  })

  it('a user who moved on while the chat was being created stays where they are (EN8)', async () => {
    const release = slowCreate()
    const agent = await runningAgent()
    const { router } = renderApp(`/chat?agent=${agent.id}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'slow create{Enter}')
    await router.navigate({ to: '/agents' })
    // The user row is saved right after create resolves (and after the redirect would fire),
    // so waiting for that POST replaces a fixed sleep.
    const saved = userRowSaved()
    release()
    await saved()
    expect(router.state.location.pathname).toBe('/agents')
  })

  it('leaving and coming back to the same URL during a slow create does not pull you into the old chat', async () => {
    const release = slowCreate()
    const agent = await runningAgent()
    const { router } = renderApp(`/chat?agent=${agent.id}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'first{Enter}')
    // Leave and come back: a new view for the same URL.
    await router.navigate({ to: '/agents', search: {} as never })
    await router.navigate({ to: '/chat', search: { agent: agent.id } as never })
    await screen.findByLabelText(/^Message /)
    const saved = userRowSaved()
    release()
    await saved()
    expect(router.state.location.pathname).toBe('/chat')
  })
})

describe('new chat: the draft', () => {
  it('a stored draft survives a cold load of /chat?agent= (the agent resolves after mount)', async () => {
    const agent = await runningAgent()
    writeDraft(ADMIN_ID, `new:${agent.id}`, 'half-written question')
    renderApp(`/chat?agent=${agent.id}`)
    await waitFor(() =>
      expect(screen.getByLabelText(/^Message /)).toHaveValue('half-written question'),
    )
  })

  it('an example over a typed draft asks first; Replace swaps it in', async () => {
    const agent = await runningAgent()
    renderApp(`/chat?agent=${agent.id}`)
    const box = await screen.findByLabelText(/^Message /)
    await userEvent.type(box, 'my own draft')
    const example = within(screen.getByRole('list', { name: 'Examples' })).getAllByRole(
      'button',
    )[0]!
    const exampleText = example.textContent!.trim()
    await userEvent.click(example)
    const dialog = await screen.findByRole('alertdialog', { name: 'Replace your draft?' })
    await userEvent.click(within(dialog).getByRole('button', { name: 'Replace' }))
    await waitFor(() => expect(screen.getByLabelText(/^Message /)).toHaveValue(exampleText))
  })

  it('Try again keeps what the user typed after the failure', async () => {
    let fail = true
    server.use(
      http.post('/api/chat/sessions', () =>
        fail ? new HttpResponse('internal error', { status: 500 }) : undefined,
      ),
    )
    const agent = await runningAgent()
    renderApp(`/chat?agent=${agent.id}`)
    const box = await screen.findByLabelText(/^Message /)
    await userEvent.type(box, 'will fail{Enter}')
    await screen.findByText('Not sent')
    await userEvent.clear(screen.getByLabelText(/^Message /))
    await userEvent.type(screen.getByLabelText(/^Message /), 'a newer thought')
    fail = false
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }))
    // Success moves into the new chat; the newer text stays in the new-chat draft, not wiped.
    await waitFor(() => expect(screen.queryByText('Not sent')).toBeNull())
    expect(readDraft(ADMIN_ID, `new:${agent.id}`)).toBe('a newer thought')
  })

  it('a Try again that fails again still keeps what the user typed after the first failure', async () => {
    server.use(
      http.post('/api/chat/sessions', () => new HttpResponse('internal error', { status: 500 })),
    )
    const agent = await runningAgent()
    renderApp(`/chat?agent=${agent.id}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'will fail{Enter}')
    await screen.findByText('Not sent')
    await userEvent.clear(screen.getByLabelText(/^Message /))
    await userEvent.type(screen.getByLabelText(/^Message /), 'a newer thought')
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await screen.findByText('Not sent')
    // Proves an absence (no late wipe of the draft after the second failure).
    await new Promise((r) => setTimeout(r, 50))
    expect(screen.getByLabelText(/^Message /)).toHaveValue('a newer thought')
  })

  it('Try again in an existing chat keeps what the user typed after the failure', async () => {
    let fail = true
    server.use(
      http.post('/api/chat/sessions/:id/messages', async ({ request }) => {
        const body = (await request.clone().json()) as { role: string }
        return fail && body.role === 'user'
          ? new HttpResponse('internal error', { status: 500 })
          : undefined
      }),
    )
    renderApp(`/chat/${DONE}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'will fail{Enter}')
    await screen.findByText('Not sent')
    await userEvent.clear(screen.getByLabelText(/^Message /))
    await userEvent.type(screen.getByLabelText(/^Message /), 'a newer thought')
    fail = false
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(screen.queryByText('Not sent')).toBeNull())
    expect(screen.getByLabelText(/^Message /)).toHaveValue('a newer thought')
  })
})
