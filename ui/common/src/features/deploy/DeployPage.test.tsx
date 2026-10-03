import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { strToU8, zipSync, type Zippable } from 'fflate'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, deployMockState } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { recordRequests, server } from '@/test/setup'
import { renderApp } from '@/test/renderApp'
import { copy } from './copy'
import { clearUploads } from './uploads'

setupPinnedSeed()
afterEach(() => {
  clearUploads()
  configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null })
})

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

async function choose(file: File) {
  await userEvent.upload(await screen.findByTestId('zip-input', {}, { timeout: 5000 }), file)
}
const itemState = (id: string) => document.getElementById(`zip-item-${id}`)?.dataset.state
const nameInput = () => screen.getByRole('textbox', { name: copy.deploy.name })
const versionInput = () => screen.getByRole('textbox', { name: copy.deploy.version })

describe('Deploy: upload a zip (plans/feat-deploy.md §4.1)', () => {
  it('checks the zip, pre-fills name and version, deploys and opens the build', async () => {
    const { router } = renderApp('/deploy')
    await choose(zipFile(GOOD))
    await waitFor(() => expect(itemState('dockerfile')).toBe('pass'))
    expect(['entrypoint', 'version', 'size'].map(itemState)).toEqual(['pass', 'pass', 'pass'])
    expect(nameInput()).toHaveValue('new-helper')
    expect(versionInput()).toHaveValue('0.3.0')
    await userEvent.click(screen.getByRole('button', { name: /^Deploy new-helper$/ }))
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/builds\/5eed000b-/), {
      timeout: 5000,
    })
    const id = router.state.location.pathname.split('/').pop()!
    expect(deployMockState().find(id)).toMatchObject({
      agentName: 'new-helper',
      record: { version_tag: '0.3.0' },
    })
  })

  it('never overwrites a name the user typed', async () => {
    renderApp('/deploy')
    await userEvent.type(
      await screen.findByRole('textbox', { name: copy.deploy.name }),
      'my-own-name',
    )
    await choose(zipFile(GOOD))
    await waitFor(() => expect(itemState('dockerfile')).toBe('pass'))
    expect(nameInput()).toHaveValue('my-own-name')
  })

  it('says why Deploy does nothing without a file, and focuses Choose file', async () => {
    renderApp('/deploy')
    await userEvent.click(await screen.findByRole('button', { name: /^Deploy/ }, { timeout: 5000 }))
    expect(await screen.findByText(copy.deploy.chooseFirst)).toBeInTheDocument()
    expect(document.activeElement).toBe(screen.getByRole('button', { name: copy.deploy.choose }))
  })

  // Changed in the /ship review: a single top folder is valid (the server flattens it), so the rejected zip here is one
  // with no Dockerfile at all.
  it('blocks a zip the server would reject, before uploading it', async () => {
    const rec = recordRequests()
    renderApp('/deploy')
    const { Dockerfile: _df, ...noDockerfile } = GOOD
    await choose(zipFile(noDockerfile))
    await waitFor(() => expect(itemState('dockerfile')).toBe('fail'))
    await userEvent.type(nameInput(), 'x')
    await userEvent.click(screen.getByRole('button', { name: /^Deploy/ }))
    expect(await screen.findByText(copy.deploy.fixFirst)).toBeInTheDocument()
    rec.stop()
    expect(rec.urls.some((u) => u.pathname === '/api/agents/upload')).toBe(false)
  })

  it('offers the next version on a 409 and resends with it', async () => {
    const agent = deployMockState().state.builds.find((b) => b.record.id.startsWith('5eed0004'))!
    const { router } = renderApp('/deploy')
    const { 'AgentCard.json': _card, ...rest } = GOOD
    await choose(
      zipFile(
        {
          ...rest,
          'AgentCard.json': JSON.stringify({
            name: agent.agentName,
            version: agent.record.version_tag,
          }),
        },
        `${agent.agentName}.zip`,
      ),
    )
    await waitFor(() => expect(versionInput()).toHaveValue(agent.record.version_tag))
    await userEvent.click(
      screen.getByRole('button', { name: new RegExp(`^Deploy ${agent.agentName}$`) }),
    )
    const use = await screen.findByRole(
      'button',
      { name: /^Use \d+\.\d+\.\d+$/ },
      { timeout: 5000 },
    )
    await waitFor(() => expect(document.activeElement).toBe(versionInput()))
    await userEvent.click(use)
    await userEvent.click(
      screen.getByRole('button', { name: new RegExp(`^Deploy ${agent.agentName}$`) }),
    )
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/builds\//), {
      timeout: 5000,
    })
  })

  it('puts a server 400 on the checklist item it names, and focuses it', async () => {
    server.use(
      http.post(
        '*/api/agents/upload',
        () =>
          new HttpResponse(
            'no Python entrypoint found (main.py, src/main.py, __main__.py, or src/__main__.py)',
            { status: 400 },
          ),
      ),
    )
    renderApp('/deploy')
    await choose(zipFile(GOOD))
    await waitFor(() => expect(itemState('dockerfile')).toBe('pass'))
    await userEvent.click(screen.getByRole('button', { name: /^Deploy/ }))
    await waitFor(() => expect(itemState('entrypoint')).toBe('fail'))
    expect(document.activeElement?.id).toBe('zip-item-entrypoint')
  })

  it('lets the user type after a server rejection (focus moves once, not on every keystroke)', async () => {
    server.use(
      http.post(
        '*/api/agents/upload',
        () =>
          new HttpResponse(
            'no Python entrypoint found (main.py, src/main.py, __main__.py, or src/__main__.py)',
            { status: 400 },
          ),
      ),
    )
    renderApp('/deploy')
    await choose(zipFile(GOOD))
    await waitFor(() => expect(itemState('dockerfile')).toBe('pass'))
    await userEvent.click(screen.getByRole('button', { name: /^Deploy/ }))
    await waitFor(() => expect(document.activeElement?.id).toBe('zip-item-entrypoint'))
    await userEvent.clear(nameInput())
    await userEvent.type(nameInput(), 'renamed')
    expect(nameInput()).toHaveValue('renamed')
    expect(document.activeElement).toBe(nameInput())
  })

  it('puts a 413 on the drop zone, not in a generic error', async () => {
    server.use(
      http.post(
        '*/api/agents/upload',
        () => new HttpResponse('upload exceeds 100 MiB', { status: 413 }),
      ),
    )
    renderApp('/deploy')
    await choose(zipFile(GOOD))
    await waitFor(() => expect(itemState('dockerfile')).toBe('pass'))
    await userEvent.click(screen.getByRole('button', { name: /^Deploy/ }))
    expect(
      await screen.findByText(copy.errors.tooLarge.problem, {}, { timeout: 5000 }),
    ).toBeInTheDocument()
    expect(screen.queryByText('upload exceeds 100 MiB')).toBeNull()
  })

  it('sends an expired session to sign-in (the shared 401 path, eng review R2)', async () => {
    server.use(
      http.post('*/api/agents/upload', () =>
        HttpResponse.json(
          { data: null, status_code: 401, message: 'missing or invalid token' },
          { status: 401 },
        ),
      ),
    )
    const { router } = renderApp('/deploy')
    await choose(zipFile(GOOD))
    await waitFor(() => expect(itemState('dockerfile')).toBe('pass'))
    await userEvent.click(screen.getByRole('button', { name: /^Deploy/ }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/login'), { timeout: 5000 })
    expect(router.state.location.search).toMatchObject({ expired: true })
  })

  it('pre-fills from the Build page (Deploy again / Deploy as vX)', async () => {
    renderApp('/deploy?name=doc-writer&version=1.3.2')
    expect(
      await screen.findByRole('textbox', { name: copy.deploy.name }, { timeout: 5000 }),
    ).toHaveValue('doc-writer')
    expect(versionInput()).toHaveValue('1.3.2')
  })

  it('shows the CLI equivalent', async () => {
    renderApp('/deploy?name=bot')
    expect(
      await screen.findByText('nasiko upload ./bot', {}, { timeout: 5000 }),
    ).toBeInTheDocument()
  })
})

describe('Build page → Deploy again', () => {
  it('links a failed build to /deploy with its name kept', async () => {
    renderApp('/builds/5eed000b-0000-4000-8000-000000000003')
    const outcome = await screen.findByTestId('build-outcome', {}, { timeout: 5000 })
    const link = within(outcome).getByRole('link', { name: copy.build.deployAgain })
    expect(link.getAttribute('href')).toMatch(/^\/deploy\?name=seed-/)
  })
})
