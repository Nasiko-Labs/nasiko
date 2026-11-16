/**
 * Attachments end to end (§7.7): the composer takes files, the turn uploads them before it
 * dispatches, the dispatch names them so the server can hand the bytes to the agent, the user row
 * claims them, and the bubble shows them — as media for the kinds a browser can play.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequestBodies, server } from '@nasiko/testing/setup'
import { clearDrafts } from './drafts'
import { clearChatRegistry } from './registry'

const png = (name = 'shot.png') => new File([new Uint8Array(8)], name, { type: 'image/png' })

interface MockAgent {
  id: string
  name: string
  display_name?: string | null
  status: string
  tags: string[]
}

async function runningAgent(): Promise<MockAgent> {
  // The seed's running agents are not all on the first page; page through as the other chat
  // page tests do.
  for (let offset = 0; offset < 1000; offset += 100) {
    const res = await fetch(new URL(`/api/agents?limit=100&offset=${offset}`, location.origin))
    const rows = (await res.json()) as MockAgent[]
    const hit = rows.find((a) => a.status === 'running' && !a.tags.includes('coding-agent'))
    if (hit) return hit
    if (rows.length < 100) break
  }
  throw new Error('no running agent in the mock seed')
}

/** Open an existing direct chat with this agent, ready to send. */
async function openChat() {
  const agent = await runningAgent()
  renderApp(`/chat?agent=${agent.id}`)
  await screen.findByLabelText(/^Message/)
  return agent
}

afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
  clearChatRegistry()
  clearDrafts()
})

describe('attaching media to a chat', () => {
  it('uploads the file, names it on the dispatch and claims it on the user row', async () => {
    const rec = recordRequestBodies()
    await openChat()
    await userEvent.upload(screen.getByTestId('attach-input'), png())
    // Chosen, not sent: the strip names it before anything leaves the browser.
    expect(
      await within(screen.getByTestId('attachment-strip')).findByText('shot.png'),
    ).toBeInTheDocument()
    expect(rec.requests.some((r) => r.url.pathname.endsWith('/files'))).toBe(false)

    await userEvent.type(screen.getByLabelText(/^Message/), 'what is in this picture')
    await userEvent.click(screen.getByRole('button', { name: 'Send' }))

    await waitFor(() =>
      expect(rec.requests.some((r) => r.url.pathname.endsWith('/files'))).toBe(true),
    )
    await rec.flush()

    const upload = rec.requests.find((r) => r.url.pathname.endsWith('/files'))
    const save = rec.requests.find((r) => r.method === 'POST' && /\/messages$/.test(r.url.pathname))
    const dispatch = rec.requests.find((r) => r.url.pathname === '/api/orchestrator/a2a')

    // The upload is a multipart POST scoped to this session, and it happens before the dispatch.
    expect(upload?.method).toBe('POST')
    expect(rec.requests.indexOf(upload!)).toBeLessThan(rec.requests.indexOf(dispatch!))

    // The ids the upload returned are the ones the row claims and the dispatch names — the same
    // file, so the server reads those bytes for the agent and the transcript keeps them.
    const claimed = (save?.body as { file_ids?: string[] }).file_ids
    const named = (dispatch?.body as { params: { metadata: { file_ids?: string[] } } }).params
      .metadata.file_ids
    expect(claimed).toHaveLength(1)
    expect(named).toEqual(claimed)
  })

  it('names the uploaded files on an Orchestrator dispatch too', async () => {
    const rec = recordRequestBodies()
    renderApp('/chat?auto=1')
    const box = await screen.findByRole('textbox')
    await userEvent.upload(screen.getByTestId('attach-input'), png())
    await userEvent.type(box, 'read the attached picture')
    await userEvent.click(screen.getByRole('button', { name: 'Send' }))

    await waitFor(() =>
      expect(rec.requests.some((r) => r.url.pathname === '/api/orchestrator/a2a')).toBe(true),
    )
    await rec.flush()

    const save = rec.requests.find((r) => r.method === 'POST' && /\/messages$/.test(r.url.pathname))
    const dispatch = rec.requests.find((r) => r.url.pathname === '/api/orchestrator/a2a')
    const metadata = (
      dispatch?.body as { params: { metadata: { agent_id?: string; file_ids?: string[] } } }
    ).params.metadata
    const claimed = (save?.body as { file_ids?: string[] }).file_ids
    expect(claimed).toHaveLength(1)
    expect(metadata.file_ids).toEqual(claimed)
    expect(metadata.agent_id).toBeUndefined()
  })

  it('shows an image back as a picture, from the authenticated file route', async () => {
    await openChat()
    await userEvent.upload(screen.getByTestId('attach-input'), png())
    await userEvent.type(screen.getByLabelText(/^Message/), 'look at this')
    await userEvent.click(screen.getByRole('button', { name: 'Send' }))

    // The name is the server's, not the browser's: undici re-serialises jsdom's FormData without
    // the filename, so this asserts the part that is ours — an image renders as an image, and its
    // bytes come from the one cookie-authenticated file route (no third-party fetch).
    const block = await screen.findByTestId('message-attachments', undefined, { timeout: 10_000 })
    const img = await within(block).findByRole('img')
    expect(img.getAttribute('src')).toMatch(
      /^(https?:\/\/[^/]+)?\/api\/chat\/files\/[^/]+\/download$/,
    )
    // And it is reachable as a download too, for a format the browser won't show.
    expect(within(block).getByRole('link')).toHaveAttribute('href', img.getAttribute('src'))
  })

  it('keeps the message and the file when the upload fails, and says so', async () => {
    server.use(
      http.post('/api/chat/sessions/:id/files', () => new HttpResponse(null, { status: 500 })),
    )
    await openChat()
    await userEvent.upload(screen.getByTestId('attach-input'), png())
    await userEvent.type(screen.getByLabelText(/^Message/), 'what is in this picture')
    await userEvent.click(screen.getByRole('button', { name: 'Send' }))

    expect(await screen.findByText(/Couldn't upload your attachments/)).toBeInTheDocument()
    // Nothing reached the agent, so the turn is not left looking sent.
    expect(screen.queryByText(/Waiting for/)).toBeNull()
  })

  it('turns away a file over the size limit and keeps the rest of the message', async () => {
    await openChat()
    const huge = new File([new Uint8Array(8)], 'huge.bin', { type: 'application/octet-stream' })
    Object.defineProperty(huge, 'size', { value: 60 * 1024 * 1024 })
    await userEvent.upload(screen.getByTestId('attach-input'), huge)
    expect(await screen.findByTestId('attach-notes')).toHaveTextContent(
      "huge.bin wasn't attached: other files must be under 10 MB.",
    )
    expect(screen.queryByTestId('attachment-strip')).toBeNull()
  })

  it('turns away a file the server would refuse over its 25 MB file limit', async () => {
    await openChat()
    const scan = new File([new Uint8Array(8)], 'scan.pdf', { type: 'application/pdf' })
    Object.defineProperty(scan, 'size', { value: 28 * 1024 * 1024 })
    await userEvent.upload(screen.getByTestId('attach-input'), scan)
    expect(await screen.findByTestId('attach-notes')).toHaveTextContent(
      "scan.pdf wasn't attached: PDFs must be under 25 MB.",
    )
  })

  it('turns away a text file over the 1 MB text limit while it is picked', async () => {
    await openChat()
    const csv = new File([new Uint8Array(8)], 'data.csv', { type: 'text/csv' })
    Object.defineProperty(csv, 'size', { value: 3 * 1024 * 1024 })
    await userEvent.upload(screen.getByTestId('attach-input'), csv)
    expect(await screen.findByTestId('attach-notes')).toHaveTextContent(
      "data.csv wasn't attached: text files must be under 1.0 MB.",
    )
    expect(screen.queryByTestId('attachment-strip')).toBeNull()
  })

  it('turns away audio over the 10 MB limit for other files', async () => {
    await openChat()
    const wav = new File([new Uint8Array(8)], 'voice.wav', { type: 'audio/wav' })
    Object.defineProperty(wav, 'size', { value: 15 * 1024 * 1024 })
    await userEvent.upload(screen.getByTestId('attach-input'), wav)
    expect(await screen.findByTestId('attach-notes')).toHaveTextContent(
      "voice.wav wasn't attached: other files must be under 10 MB.",
    )
  })

  it('names every per-type limit on the attach button', async () => {
    await openChat()
    expect(screen.getByRole('button', { name: 'Attach files' })).toHaveAttribute(
      'title',
      'Up to 10 files: images and PDFs 25 MB, text files 1.0 MB, other files 10 MB each',
    )
  })

  it('keeps one message within the 30 MB the server accepts', async () => {
    await openChat()
    const sized = (name: string, mb: number) => {
      const f = new File([new Uint8Array(8)], name, { type: 'application/pdf' })
      Object.defineProperty(f, 'size', { value: mb * 1024 * 1024 })
      return f
    }
    await userEvent.upload(screen.getByTestId('attach-input'), [
      sized('a.pdf', 20),
      sized('b.pdf', 12),
    ])
    expect(await screen.findByTestId('attach-notes')).toHaveTextContent(
      "b.pdf wasn't attached: one message can carry 30 MB of files in total.",
    )
    expect(screen.getByTestId('attachment-strip')).toHaveTextContent('a.pdf')
  })

  it('removes an attachment before sending', async () => {
    await openChat()
    await userEvent.upload(screen.getByTestId('attach-input'), png())
    await userEvent.click(await screen.findByRole('button', { name: 'Remove shot.png' }))
    expect(screen.queryByTestId('attachment-strip')).toBeNull()
  })
})
