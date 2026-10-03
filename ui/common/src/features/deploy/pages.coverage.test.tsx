/**
 * Ship-audit gap tests for the deploy pages: error states with Retry, the stream's 401 path, the Active filter and
 * paging, GitHub clone errors on the field they name, upload-form branches, and the failed-build toast.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { strToU8, zipSync, type Zippable } from 'fflate'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, deployMockState } from '@/mocks/handlers'
import { buildId, readMultipart } from '@/mocks/deploy'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequests, server } from '@/test/setup'
import { copy } from './copy'
import { pollOnce } from './follower'
import { clearUploads } from './uploads'

setupPinnedSeed()
afterEach(() => {
  clearUploads()
  configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null })
})

const T = { timeout: 5000 }
const couldntLoad = (what: string) => screen.findByText(`Couldn't load ${what}`, {}, T)
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

describe('Build page errors', () => {
  it('shows a failed build read with Retry, which recovers', async () => {
    let fail = true
    server.use(http.get('*/api/builds/:id', () => (fail ? text('db down', 500) : undefined)))
    renderApp(`/builds/${buildId(3)}`)
    await couldntLoad(copy.build.what)
    fail = false
    await userEvent.click(screen.getByRole('button', { name: /Retry/ }))
    expect(await screen.findByTestId('build-outcome', {}, T)).toHaveAttribute(
      'data-outcome',
      'failed',
    )
  })

  it("sends a 401 on the deploy stream through the app's one expiry path (re-checks me)", async () => {
    let expired = false
    server.use(
      http.get('*/api/agents/deploys/:id/stream', () => {
        expired = true
        return text('missing or invalid token', 401)
      }),
      http.get('*/api/me', () =>
        expired
          ? HttpResponse.json(
              { data: null, status_code: 401, message: 'missing or invalid token' },
              { status: 401 },
            )
          : undefined,
      ),
    )
    const { router } = renderApp(`/builds/${buildId(1)}`)
    await waitFor(() => expect(router.state.location.pathname).toBe('/login'), T)
    expect(router.state.location.search).toMatchObject({ expired: true })
  })
})

describe('Builds list', () => {
  const rows = async () =>
    within(await screen.findByTestId('builds-table', {}, T)).getAllByTestId('build-row')

  it('shows a failed list with Retry, which recovers', async () => {
    let fail = true
    server.use(http.get('*/api/builds', () => (fail ? text('db down', 500) : undefined)))
    renderApp('/builds')
    await couldntLoad(copy.builds.what)
    fail = false
    await userEvent.click(screen.getByRole('button', { name: /Retry/ }))
    expect((await rows()).length).toBeGreaterThan(2)
  })

  it('In progress shows only the pinned queued and building builds, with no paging', async () => {
    renderApp('/builds?status=active')
    const r = await rows()
    expect(r.map((x) => x.dataset.status).sort()).toEqual(['building', 'queued'])
    expect(screen.queryByRole('button', { name: copy.builds.next })).toBeNull()
  })

  it('pages the list: Next pushes page=1, Previous goes back to the first page', async () => {
    expect(deployMockState().state.builds.length).toBeGreaterThan(22)
    const { router } = renderApp('/builds?status=success')
    const first = (await rows()).map((x) => within(x).getByRole('link').getAttribute('href'))
    expect(first).toHaveLength(20)
    await userEvent.click(screen.getByRole('button', { name: copy.builds.next }))
    await waitFor(() => expect(router.state.location.search).toMatchObject({ page: 1 }), T)
    await waitFor(
      async () =>
        expect(
          (await rows()).map((x) => within(x).getByRole('link').getAttribute('href'))[0],
        ).not.toBe(first[0]),
      T,
    )
    await userEvent.click(screen.getByRole('button', { name: copy.builds.previous }))
    await waitFor(() => expect(router.state.location.search.page).toBeUndefined(), T)
  })
})

describe("Agent's Builds tab errors", () => {
  it('shows a failed read with Retry', async () => {
    server.use(http.get('*/api/builds/agent/:agentId', () => text('db down', 500)))
    renderApp(`/agents/${seed.agents[0]!.id}?tab=builds`)
    await couldntLoad(copy.agentBuilds.what)
    expect(screen.getByRole('button', { name: /Retry/ })).toBeInTheDocument()
  })
})

describe('Deploy from GitHub: errors', () => {
  const pick = async (fullName: string) => {
    const r = await screen.findAllByTestId('repo-row', {}, T)
    await userEvent.click(
      within(r.find((x) => x.textContent?.includes(fullName))!).getByRole('radio'),
    )
  }

  it('shows a failed GitHub status read with Retry', async () => {
    server.use(http.get('*/api/auth/github/status', () => text('boom', 500)))
    renderApp('/deploy?method=github')
    await couldntLoad(copy.github.what)
  })

  it('explains a 403 clone (GitHub disconnected meanwhile) in words, not server text', async () => {
    server.use(
      http.post('*/api/github/clone', () =>
        text('GitHub not connected — visit /agents.html?view=import to connect', 403),
      ),
    )
    renderApp('/deploy?method=github')
    await pick('acme/fresh-agent')
    await userEvent.click(screen.getByRole('button', { name: 'Deploy acme/fresh-agent' }))
    expect(
      await screen.findByText(copy.errors.githubDisconnected.problem, {}, T),
    ).toBeInTheDocument()
    expect(screen.queryByText(/agents\.html/)).toBeNull()
  })

  it("puts the server's invalid-branch 422 under the Branch field", async () => {
    const { router } = renderApp('/deploy?method=github')
    await pick('acme/fresh-agent')
    const branch = screen.getByRole('textbox', { name: copy.github.branch })
    await userEvent.clear(branch)
    await userEvent.type(branch, 'bad..branch')
    await userEvent.click(screen.getByRole('button', { name: 'Deploy acme/fresh-agent' }))
    expect(
      await screen.findByText(/invalid branch name 'bad\.\.branch'/, {}, T),
    ).toBeInTheDocument()
    expect(branch).toHaveAttribute('aria-invalid', 'true')
    expect(screen.queryByText(copy.deploy.failed)).toBeNull()
    expect(router.state.location.pathname).toBe('/deploy')
  })
})

describe('Deploy from a registry: errors', () => {
  it('shows a server failure outside the field, with its text', async () => {
    server.use(http.post('*/api/import/registry', () => text('pull failed: manifest unknown', 500)))
    renderApp('/deploy?method=registry')
    await userEvent.type(
      await screen.findByRole('textbox', { name: copy.registry.reference }, T),
      'registry.nasiko.dev/acme/bot:1.0.0',
    )
    await userEvent.click(screen.getByRole('button', { name: copy.registry.submit }))
    expect(await screen.findByText(copy.deploy.failed, {}, T)).toBeInTheDocument()
    expect(screen.getByText('pull failed: manifest unknown')).toBeInTheDocument()
    expect(screen.getByRole('textbox', { name: copy.registry.reference })).not.toHaveAttribute(
      'aria-invalid',
      'true',
    )
  })
})

describe('Deploy: upload form branches', () => {
  it('refuses a file that is not a .zip, without reading it', async () => {
    renderApp('/deploy')
    // userEvent.upload honours `accept`; applyAccept off simulates a drop or a browser that ignores it.
    const input = await screen.findByTestId('zip-input', {}, T)
    await userEvent.setup({ applyAccept: false }).upload(input, new File(['x'], 'agent.tar.gz'))
    expect(await screen.findByText(copy.deploy.zipOnly)).toBeInTheDocument()
    expect(screen.queryByTestId('zip-file')).toBeNull()
  })

  it('asks for a version when neither the zip nor the form has one, and sends nothing', async () => {
    const rec = recordRequests()
    renderApp('/deploy')
    const { 'AgentCard.json': _card, ...noVersion } = GOOD
    await choose(zipFile(noVersion, 'plain-bot.zip'))
    await waitFor(() => expect(itemState('dockerfile')).toBe('pass'), T)
    await userEvent.click(screen.getByRole('button', { name: /^Deploy plain-bot$/ }))
    expect(await screen.findByText(copy.deploy.versionNeeded)).toBeInTheDocument()
    expect(document.activeElement).toBe(screen.getByRole('textbox', { name: copy.deploy.version }))
    rec.stop()
    expect(rec.urls.some((u) => u.pathname === '/api/agents/upload')).toBe(false)
  })

  it('shows a 500 as "Couldn\'t deploy" with the server text, and sends the touched advanced fields', async () => {
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
    await userEvent.click(screen.getByRole('button', { name: copy.deploy.addEnv }))
    await userEvent.type(
      screen.getByRole('textbox', { name: `${copy.deploy.envKey} 1` }),
      'API_URL',
    )
    await userEvent.type(
      screen.getByRole('textbox', { name: `${copy.deploy.envValue} 1` }),
      'https://x',
    )
    await userEvent.click(screen.getByRole('switch', { name: copy.deploy.storage }))
    await userEvent.type(screen.getByRole('textbox', { name: copy.deploy.storagePath }), '/data')
    await userEvent.click(screen.getByRole('button', { name: /^Deploy new-helper$/ }))
    expect(await screen.findByText(copy.deploy.failed, {}, T)).toBeInTheDocument()
    expect(screen.getByText('docker daemon unavailable')).toBeInTheDocument()
    const fd = sent as Map<string, string | Blob> | null
    expect(fd?.get('env')).toBe('{"API_URL":"https://x"}')
    expect(fd?.get('writable')).toBe('true')
    expect(fd?.get('writable_path')).toBe('/data')
    expect(fd?.has('inbound_format')).toBe(false)
  })
})

describe('Background follow: a failed build', () => {
  it('toasts "<name> failed" once it finishes elsewhere, and See why opens its Build page', async () => {
    configureMocks({ variant: 'deploy-build-fails' })
    const { router } = renderApp('/deploy?method=github')
    const r = await screen.findAllByTestId('repo-row', {}, T)
    await userEvent.click(
      within(r.find((x) => x.textContent?.includes('acme/fresh-agent'))!).getByRole('radio'),
    )
    await userEvent.click(screen.getByRole('button', { name: 'Deploy acme/fresh-agent' }))
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/builds\//), T)
    const id = router.state.location.pathname.split('/').pop()!
    await router.navigate({ to: '/agents' })
    deployMockState().advance(id, 'failed', 'failed')
    await pollOnce()
    expect(await screen.findByText(copy.toast.failed('fresh-agent'), {}, T)).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: copy.toast.seeWhy }))
    await waitFor(() => expect(router.state.location.pathname).toBe(`/builds/${id}`), T)
  })
})
