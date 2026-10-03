/**
 * Direct chat errors and notices on the page: the §6.6 rejected-before-run errors (403, 404, 429,
 * 503), an agent failure, a definite save failure, an empty reply, a send while the last reply is
 * unconfirmed, and a history load failure.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureChatMock } from '@/mocks/chatStore'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { clearDrafts } from './drafts'
import { clearChatRegistry } from './registry'

const DONE = '5eedc000-0000-4000-8000-00000000c001'
const LOST = '5eedc000-0000-4000-8000-00000000c002'

/** Answer every direct dispatch with a JSON-RPC error, counting calls. */
function rejectDispatch(status: number, code: number, message: string) {
  const calls = { n: 0 }
  server.use(
    http.post('/api/orchestrator/a2a', () => {
      calls.n++
      return HttpResponse.json({ jsonrpc: '2.0', id: null, error: { code, message } }, { status })
    }),
  )
  return calls
}

/** Send in the saved chat and wait for a notice. Tests re-query it: the turn remounts when history refetches. */
async function sendInDone(text: string) {
  renderApp(`/chat/${DONE}`)
  await userEvent.type(await screen.findByLabelText(/^Message /), `${text}{Enter}`)
  await screen.findByTestId('error-notice')
}

afterEach(() => {
  clearChatRegistry()
  clearDrafts()
})

describe('rejected before run (§6.6)', () => {
  it('403 links to the agent and locks the composer until the tab regains focus', async () => {
    rejectDispatch(403, -32605, 'forbidden')
    await sendInDone('hello')
    await waitFor(() =>
      expect(
        within(screen.getByTestId('error-notice')).getByText("You can't use this agent."),
      ).toBeInTheDocument(),
    )
    await waitFor(() =>
      expect(
        within(screen.getByTestId('error-notice'))
          .getByRole('link', { name: 'Open the agent' })
          .getAttribute('href'),
      ).toMatch(/^\/agents\/[0-9a-f-]{36}/),
    )
    await userEvent.type(screen.getByLabelText(/^Message /), 'again')
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled()
    fireEvent.focus(window)
    await waitFor(() => expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled())
  })

  it('404 says the agent is gone and points at Agents, with the server detail', async () => {
    rejectDispatch(404, -32604, "agent 'x' not found or not running")
    await sendInDone('hello')
    await waitFor(() =>
      expect(
        within(screen.getByTestId('error-notice')).getByText(
          'This agent is gone or you lost access.',
        ),
      ).toBeInTheDocument(),
    )
    await waitFor(() =>
      expect(
        within(screen.getByTestId('error-notice')).getByRole('link', { name: 'Agents' }),
      ).toHaveAttribute('href', '/agents'),
    )
    await waitFor(() =>
      expect(
        within(screen.getByTestId('error-notice')).getByText(
          /Server said: agent 'x' not found or not running · HTTP 404 · code -32604/,
        ),
      ).toBeInTheDocument(),
    )
  })

  it('429 offers Retry, which dispatches again without asking', async () => {
    const calls = rejectDispatch(429, -32000, 'rate limited')
    await sendInDone('hello')
    await waitFor(() =>
      expect(
        within(screen.getByTestId('error-notice')).getByText(
          'Too many messages. Try again in a minute.',
        ),
      ).toBeInTheDocument(),
    )
    await userEvent.click(
      within(screen.getByTestId('error-notice')).getByRole('button', { name: 'Retry' }),
    )
    await waitFor(() => expect(calls.n).toBe(2))
    expect(screen.queryByRole('alertdialog')).toBeNull()
  })

  it('503 says there is no running agent and points at Agents', async () => {
    rejectDispatch(503, -32603, 'no agents available')
    await sendInDone('hello')
    await waitFor(() =>
      expect(
        within(screen.getByTestId('error-notice')).getByText(
          'OpenRuntime has no running agent for this.',
        ),
      ).toBeInTheDocument(),
    )
    await waitFor(() =>
      expect(
        within(screen.getByTestId('error-notice')).getByRole('link', { name: 'Agents' }),
      ).toHaveAttribute('href', '/agents'),
    )
  })
})

describe('turn outcomes', () => {
  it('an agent that fails the task shows the failure with Agent logs', async () => {
    configureChatMock({ scenario: 'failed' })
    await sendInDone('do it')
    await waitFor(() =>
      expect(
        within(screen.getByTestId('error-notice')).getByText('The agent reported an error.'),
      ).toBeInTheDocument(),
    )
    await waitFor(() =>
      expect(
        within(screen.getByTestId('error-notice')).getByRole('button', { name: 'Refresh status' }),
      ).toBeInTheDocument(),
    )
    await waitFor(() =>
      expect(
        within(screen.getByTestId('error-notice'))
          .getByRole('link', { name: 'Agent logs' })
          .getAttribute('href'),
      ).toContain('tab=activity'),
    )
  })

  it('a definite (4xx) save failure offers Save again first; Discard drops the reply', async () => {
    server.use(
      http.post('/api/chat/sessions/:id/messages', async ({ request }) => {
        const body = (await request.clone().json()) as { role: string }
        return body.role === 'assistant'
          ? HttpResponse.json({ message: 'row rejected' }, { status: 422 })
          : undefined
      }),
    )
    await sendInDone('One more')
    await waitFor(() =>
      expect(
        within(screen.getByTestId('error-notice')).getByText('Reply not saved.'),
      ).toBeInTheDocument(),
    )
    await waitFor(() =>
      expect(
        within(screen.getByTestId('error-notice'))
          .getAllByRole('button')
          .map((b) => b.textContent?.trim())
          .slice(0, 2),
      ).toEqual(['Save again', 'Discard reply']),
    )
    // History refetches once the turn ends and remounts the turn: re-query the button until the click lands.
    await waitFor(() => {
      const discard = screen.queryByRole('button', { name: 'Discard reply' })
      if (discard) fireEvent.click(discard)
      expect(screen.queryByText('Reply not saved.')).toBeNull()
    })
  })

  it('an empty reply shows the no-reply notice with Refresh status and Run again (ISSUE-004)', async () => {
    configureChatMock({ scenario: 'empty-reply' })
    renderApp(`/chat/${DONE}`)
    await userEvent.type(await screen.findByLabelText(/^Message /), 'anything{Enter}')
    // Re-query inside waitFor: the turn remounts when history refetches, replacing the node.
    await waitFor(() => {
      const notice = screen.getByTestId('error-notice')
      expect(notice).toHaveTextContent('No reply has been saved for this message yet.')
      expect(within(notice).getByRole('button', { name: 'Refresh status' })).toBeInTheDocument()
      expect(within(notice).getByRole('button', { name: 'Run again' })).toBeInTheDocument()
    })
  })

  it('sending after an unconfirmed reply asks first; Send anyway dispatches', async () => {
    let dispatched = 0
    server.events.on('request:start', ({ request }) => {
      if (request.url.endsWith('/api/orchestrator/a2a')) dispatched++
    })
    try {
      renderApp(`/chat/${LOST}`)
      await screen.findByText('No reply has been saved for this message yet.')
      await userEvent.type(screen.getByLabelText(/^Message /), 'still there?{Enter}')
      const dialog = await screen.findByRole('alertdialog', {
        name: 'Your last message may still be running.',
      })
      expect(dispatched).toBe(0)
      await userEvent.click(within(dialog).getByRole('button', { name: 'Send anyway' }))
      await waitFor(() => expect(dispatched).toBe(1))
    } finally {
      server.events.removeAllListeners()
    }
  })
})

describe('load failures', () => {
  it('a history failure that is not a 404 says so and offers Retry', async () => {
    let calls = 0
    server.use(
      http.get('/api/chat/sessions/:id/messages', () => {
        calls++
        return new HttpResponse('boom', { status: 500 })
      }),
    )
    renderApp(`/chat/${DONE}`)
    expect(await screen.findByText("Couldn't load this chat.")).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }))
    await waitFor(() => expect(calls).toBe(2))
  })
})
