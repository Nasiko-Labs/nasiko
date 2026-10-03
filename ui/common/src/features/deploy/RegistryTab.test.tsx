import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { delay, http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, deployMockState } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { recordRequestBodies, recordRequests, server } from '@/test/setup'
import { renderApp } from '@/test/renderApp'
import { copy } from './copy'

const buildCopy = copy.build
import { parseReference } from './registry'
import { clearUploads } from './uploads'

setupPinnedSeed()
afterEach(() => {
  clearUploads()
  configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null })
})

const field = () =>
  screen.findByRole('textbox', { name: copy.registry.reference }, { timeout: 5000 })
async function importRef(ref: string) {
  await userEvent.type(await field(), ref)
  await userEvent.click(screen.getByRole('button', { name: copy.registry.submit }))
}

describe('parseReference', () => {
  it('reads host/owner/name[:tag] and rejects anything without a real host', () => {
    expect(parseReference('registry.nasiko.dev/acme/bot:1.2.0')).toEqual({
      host: 'registry.nasiko.dev',
      repo: 'acme/bot',
      tag: '1.2.0',
    })
    expect(parseReference('https://registry.nasiko.dev/a/b/c')).toEqual({
      host: 'registry.nasiko.dev',
      repo: 'a/b/c',
      tag: 'latest',
    })
    expect(parseReference('localhost:5000/acme/bot')).toMatchObject({ host: 'localhost:5000' })
    for (const bad of [
      'acme/bot',
      'registry.nasiko.dev/bot',
      'registry.nasiko.dev/acme/bot:',
      'not a ref',
      '',
    ])
      expect(parseReference(bad)).toBeNull()
  })
})

describe('Deploy: from a registry (plans/feat-deploy.md §4.3)', () => {
  it('rejects a malformed reference before sending', async () => {
    const rec = recordRequests()
    renderApp('/deploy?method=registry')
    await importRef('acme/bot')
    expect(await screen.findByText(copy.registry.format)).toBeInTheDocument()
    rec.stop()
    expect(rec.urls.some((u) => u.pathname === '/api/import/registry')).toBe(false)
  })

  it('opens the build page for an agent package it built', async () => {
    const { router } = renderApp('/deploy?method=registry')
    await importRef('registry.nasiko.dev/nasiko/packaged-bot:0.5.0')
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/builds\/5eed000b-/), {
      timeout: 5000,
    })
  })

  it('opens the agent page for a plain image it pulled (no build)', async () => {
    const { router } = renderApp('/deploy?method=registry')
    await importRef('registry.nasiko.dev/acme/pulled-bot:1.0.0')
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/agents\/5eed000c-/), {
      timeout: 5000,
    })
  })

  it('puts a disallowed host on the field', async () => {
    renderApp('/deploy?method=registry')
    await importRef('ghcr.io/acme/bot:1.0.0')
    expect(
      await screen.findByText(
        "registry host 'ghcr.io' is not in the allowed list",
        {},
        { timeout: 5000 },
      ),
    ).toBeInTheDocument()
  })

  it('puts a version the agent already built on the field (the build-from-source path)', async () => {
    const b = deployMockState().state.builds[0]!
    renderApp('/deploy?method=registry')
    await importRef(`registry.nasiko.dev/nasiko/${b.agentName}:${b.record.version_tag}`)
    expect(
      await screen.findByText(/already exists in this agent's history/, {}, { timeout: 5000 }),
    ).toBeInTheDocument()
  })

  it('sends the reference without a scheme (the server splits the host at the first slash)', async () => {
    const rec = recordRequestBodies()
    const { router } = renderApp('/deploy?method=registry')
    await importRef('https://registry.nasiko.dev/acme/pulled-bot:1.0.0')
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/agents\//), {
      timeout: 5000,
    })
    rec.stop()
    expect(rec.requests.find((r) => r.url.pathname === '/api/import/registry')!.body).toEqual({
      reference: 'registry.nasiko.dev/acme/pulled-bot:1.0.0',
    })
  })

  it('toasts once when an import finishes after leaving the Registry tab (R4)', async () => {
    let release!: () => void
    server.use(
      http.post('*/api/import/registry', async () => {
        await new Promise<void>((r) => {
          release = r
        })
        return HttpResponse.json(
          { agent_id: 'a1', build_id: null, container_name: 'agent-slow-bot', status: 'success' },
          { status: 201 },
        )
      }),
    )
    const { router } = renderApp('/deploy?method=registry')
    await importRef('registry.nasiko.dev/acme/slow-bot:1.0.0')
    await screen.findByTestId('import-progress', {}, { timeout: 5000 })
    await router.navigate({ to: '/builds' })
    release()
    expect(
      await screen.findByText(buildCopy.running('slow-bot'), {}, { timeout: 5000 }),
    ).toBeInTheDocument()
    expect(router.state.location.pathname).toBe('/builds')
    expect(screen.getAllByText(buildCopy.running('slow-bot'))).toHaveLength(1)
  })

  it('says so when registry import is turned off, and offers Upload', async () => {
    configureMocks({ variant: 'deploy-registry-disabled' })
    renderApp('/deploy?method=registry')
    await importRef('registry.nasiko.dev/acme/bot:1.0.0')
    expect(
      await screen.findByText(copy.registry.disabled, {}, { timeout: 5000 }),
    ).toBeInTheDocument()
    expect(screen.getByRole('link', { name: copy.github.useUpload }).getAttribute('href')).toBe(
      '/deploy?method=upload',
    )
  })

  it('says the agent did not start when the deploy failed (container_name null)', async () => {
    configureMocks({ variant: 'deploy-registry-not-running' })
    renderApp('/deploy?method=registry')
    await importRef('registry.nasiko.dev/acme/lazy-bot:1.0.0')
    const alert = await screen.findByTestId('import-not-running', {}, { timeout: 5000 })
    expect(
      within(alert).getByRole('link', { name: copy.registry.openAgent }).getAttribute('href'),
    ).toMatch(/^\/agents\//)
  })

  it('shows one honest step while importing, and keeps going after leaving the page (eng review R4)', async () => {
    server.use(
      http.post('*/api/import/registry', async () => {
        await delay(1_500)
        return HttpResponse.json(
          { agent_id: 'a1', build_id: null, container_name: 'agent-x', status: 'success' },
          { status: 201 },
        )
      }),
    )
    const { router } = renderApp('/deploy?method=registry')
    await importRef('registry.nasiko.dev/acme/slow-bot:1.0.0')
    const progress = await screen.findByTestId('import-progress', {}, { timeout: 5000 })
    expect(within(progress).getAllByRole('listitem')).toHaveLength(2)
    expect(within(progress).getByText(copy.registry.keepUsing)).toBeInTheDocument()
    await router.navigate({ to: '/builds' })
    await router.navigate({ to: '/deploy', search: { method: 'registry' } })
    expect(await screen.findByTestId('import-progress', {}, { timeout: 5000 })).toBeInTheDocument()
    await waitFor(() => expect(router.state.location.pathname).toBe('/agents/a1'), {
      timeout: 5000,
    })
  })
})
