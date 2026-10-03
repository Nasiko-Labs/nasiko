/**
 * Ship-audit gap tests, second pass: GitHub account switch, expired connection, repository and clone errors, the upload
 * form's pre-submit version suggestion, inbound format and Cancel, the slow-build hint, and the not-running toast.
 */
import { act, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { strToU8, zipSync, type Zippable } from 'fflate'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { configureMocks, deployMockState } from '@/mocks/handlers'
import { buildId, readMultipart } from '@/mocks/deploy'
import { FIXED, now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { copy } from './copy'
import { importFinished } from './follower'
import { SLOW_BUILD_MS } from './tuning'
import { clearUploads, setXhrFactory, type XhrFactory } from './uploads'

setupPinnedSeed()
let prevXhr: XhrFactory | null = null
afterEach(() => {
  clearUploads()
  if (prevXhr) setXhrFactory(prevXhr)
  prevXhr = null
  vi.setSystemTime(FIXED)
  configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null })
})

const T = { timeout: 5000 }
const text = (body: string, status: number) =>
  new HttpResponse(body, { status, headers: { 'Content-Type': 'text/plain' } })
const zipFile = (files: Record<string, string>, name = 'support-bot.zip') =>
  new File(
    [
      zipSync(
        Object.fromEntries(Object.entries(files).map(([k, v]) => [k, strToU8(v)])) as Zippable,
      ),
    ],
    name,
    { type: 'application/zip' },
  )
const GOOD = {
  Dockerfile: 'FROM python:3.12\n',
  'main.py': 'print(1)\n',
  'AgentCard.json': JSON.stringify({ name: 'new-helper', version: '0.3.0' }),
}
const choose = async (f: File) => userEvent.upload(await screen.findByTestId('zip-input', {}, T), f)
const itemState = (id: string) => document.getElementById(`zip-item-${id}`)?.dataset.state
const pick = async (fullName: string) => {
  const r = await screen.findAllByTestId('repo-row', {}, T)
  await userEvent.click(
    within(r.find((x) => x.textContent?.includes(fullName))!).getByRole('radio'),
  )
}

describe('Deploy from GitHub: connection states', () => {
  it('Switch account signs out of GitHub, then opens the connect popup', async () => {
    const open = vi.spyOn(window, 'open').mockReturnValue({ closed: false } as Window)
    let loggedOut = false
    server.use(
      http.delete('*/api/github/logout', () => {
        loggedOut = true
        return undefined
      }),
    )
    renderApp('/deploy?method=github')
    await userEvent.click(await screen.findByRole('button', { name: copy.github.switchAccount }, T))
    await waitFor(
      () =>
        expect(open).toHaveBeenCalledWith(
          expect.stringContaining('about:blank'),
          'openruntime-github',
          expect.any(String),
        ),
      T,
    )
    expect(loggedOut).toBe(true)
    expect(deployMockState().state.github.connected).toBe(false)
  })

  it('says an expired connection (connected, valid:false) needs Reconnect GitHub', async () => {
    server.use(
      http.get('*/api/github/user', () =>
        HttpResponse.json({ connected: true, valid: false, login: 'octocat' }),
      ),
    )
    renderApp('/deploy?method=github')
    expect(
      await screen.findByRole('button', { name: copy.github.reconnect }, T),
    ).toBeInTheDocument()
    expect(screen.getByText(copy.github.invalid)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: copy.github.connect })).toBeNull()
    expect(screen.queryByTestId('repo-row')).toBeNull()
  })

  it('shows a failed repositories read with Retry, which recovers', async () => {
    let fail = true
    server.use(
      http.get('*/api/github/repositories', () =>
        fail ? text('github api down', 500) : undefined,
      ),
    )
    renderApp('/deploy?method=github')
    expect(await screen.findByText(`Couldn't load ${copy.github.what}`, {}, T)).toBeInTheDocument()
    fail = false
    await userEvent.click(screen.getByRole('button', { name: /Retry/ }))
    expect((await screen.findAllByTestId('repo-row', {}, T)).length).toBeGreaterThan(2)
  })

  it("puts a clone 400's server text under the Name field", async () => {
    server.use(
      http.post('*/api/github/clone', () => text('agent name already taken by another owner', 400)),
    )
    const { router } = renderApp('/deploy?method=github')
    await pick('acme/fresh-agent')
    await userEvent.click(screen.getByRole('button', { name: 'Deploy acme/fresh-agent' }))
    expect(
      await screen.findByText('agent name already taken by another owner', {}, T),
    ).toBeInTheDocument()
    expect(screen.getByRole('textbox', { name: copy.deploy.name })).toHaveAttribute(
      'aria-invalid',
      'true',
    )
    expect(screen.queryByText(copy.deploy.failed)).toBeNull()
    expect(router.state.location.pathname).toBe('/deploy')
  })
})

/** A stand-in XMLHttpRequest (uploads.test.ts), so the upload stays in flight until the test says otherwise. */
class FakeXhr {
  status = 0
  responseText = ''
  withCredentials = false
  upload: { onprogress: unknown } = { onprogress: null }
  onload: (() => void) | null = null
  onerror: (() => void) | null = null
  onabort: (() => void) | null = null
  aborted = false
  open() {}
  setRequestHeader() {}
  send() {}
  abort() {
    this.aborted = true
    this.onabort?.()
  }
}

describe('Deploy: upload form, second pass', () => {
  it("suggests the next version for an existing agent's name before any 409, and Use fills it in", async () => {
    const existing = deployMockState().state.builds.find((b) =>
      b.record.id.startsWith('5eed0004'),
    )!.agentName
    renderApp('/deploy')
    await userEvent.type(
      await screen.findByRole('textbox', { name: copy.deploy.name }, T),
      existing,
    )
    const hint = await screen.findByText(
      /^Current version \d+\.\d+\.\d+, next suggested \d+\.\d+\.\d+$/,
      {},
      T,
    )
    const next = hint.textContent!.match(/next suggested (\S+)$/)![1]!
    await userEvent.click(screen.getByRole('button', { name: copy.deploy.use(next) }))
    expect(screen.getByRole('textbox', { name: copy.deploy.version })).toHaveValue(next)
    // The suggestion is now the version: the button goes away.
    expect(screen.queryByRole('button', { name: copy.deploy.use(next) })).toBeNull()
  })

  it('sends the chosen inbound format as inbound_format', async () => {
    let sent: Map<string, string | Blob> | null = null
    server.use(
      http.post('*/api/agents/upload', async ({ request }) => {
        sent = await readMultipart(request)
        return text('docker daemon unavailable', 500)
      }),
    )
    renderApp('/deploy')
    await choose(zipFile(GOOD))
    await waitFor(() => expect(itemState('dockerfile')).toBe('pass'), T)
    await userEvent.click(screen.getByRole('button', { name: copy.deploy.advanced }))
    await userEvent.click(screen.getByRole('combobox', { name: copy.deploy.inbound }))
    await userEvent.click(await screen.findByRole('option', { name: 'anthropic' }))
    await userEvent.click(screen.getByRole('button', { name: /^Deploy new-helper$/ }))
    expect(await screen.findByText(copy.deploy.failed, {}, T)).toBeInTheDocument()
    expect((sent as Map<string, string | Blob> | null)?.get('inbound_format')).toBe('anthropic')
  })

  it('Cancel stops an upload in flight and says it was cancelled', async () => {
    let xhr: FakeXhr | null = null
    prevXhr = setXhrFactory(() => (xhr = new FakeXhr()) as unknown as XMLHttpRequest)
    const { router } = renderApp('/deploy')
    await choose(zipFile(GOOD))
    await waitFor(() => expect(itemState('dockerfile')).toBe('pass'), T)
    await userEvent.click(screen.getByRole('button', { name: /^Deploy new-helper$/ }))
    await screen.findByTestId('upload-progress', {}, T)
    await userEvent.click(screen.getByRole('button', { name: copy.deploy.cancel }))
    expect(await screen.findByText(copy.deploy.cancelled, {}, T)).toBeInTheDocument()
    expect((xhr as FakeXhr | null)?.aborted).toBe(true)
    expect(screen.queryByTestId('upload-progress')).toBeNull()
    expect(screen.getByRole('button', { name: /^Deploy new-helper$/ })).toBeInTheDocument()
    expect(router.state.location.pathname).toBe('/deploy')
  })
})

describe('Build page: slow build', () => {
  it(`says "Taking longer than usual" once a running build passes ${SLOW_BUILD_MS / 60_000} min`, async () => {
    // The mocks keep the pinned clock (the build stays running); the page's clock is past the slow mark.
    vi.setSystemTime(FIXED.getTime() + SLOW_BUILD_MS + 60 * 60_000)
    renderApp(`/builds/${buildId(1)}`)
    const list = await screen.findByRole('list', { name: copy.build.stepsLabel }, T)
    expect(
      await within(list).findByText(
        /Taking longer than usual · builds time out after 30 min/,
        {},
        T,
      ),
    ).toBeInTheDocument()
  })
})

describe('Background follow: a registry import that did not start', () => {
  it('toasts "<name> was imported, but did not start", and Open agent opens the agent', async () => {
    const agent = seed.agents[0]!
    const { router } = renderApp('/builds')
    await screen.findByTestId('builds-table', {}, T)
    act(() => {
      expect(importFinished({ kind: 'notRunning', agentId: agent.id, name: 'imported-bot' })).toBe(
        true,
      )
    })
    expect(
      await screen.findByText(copy.toast.notRunning('imported-bot'), {}, T),
    ).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: copy.build.openAgent }))
    await waitFor(() => expect(router.state.location.pathname).toBe(`/agents/${agent.id}`), T)
  })
})
