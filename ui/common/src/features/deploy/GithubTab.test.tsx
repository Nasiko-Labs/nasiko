import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { configureMocks, deployMockState } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { recordRequestBodies, server } from '@/test/setup'
import { renderApp } from '@/test/renderApp'
import { copy } from './copy'
import { clearUploads, clonedRepo } from './uploads'

setupPinnedSeed()
afterEach(() => {
  // The follower and upload registry are module state: a followed build must not poll into the next test.
  clearUploads()
  configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null })
})

const repoRows = async () => screen.findAllByTestId('repo-row', {}, { timeout: 5000 })
const pick = async (fullName: string) => {
  const rows = await repoRows()
  const row = rows.find((r) => r.textContent?.includes(fullName))!
  await userEvent.click(within(row).getByRole('radio'))
}

describe('Deploy: from GitHub (plans/feat-deploy.md §4.2)', () => {
  it('says so and offers Upload when the server has no GitHub OAuth app', async () => {
    configureMocks({ variant: 'deploy-github-unconfigured' })
    renderApp('/deploy?method=github')
    expect(
      await screen.findByText(copy.github.notConfigured, {}, { timeout: 5000 }),
    ).toBeInTheDocument()
    expect(screen.getByRole('link', { name: copy.github.useUpload }).getAttribute('href')).toBe(
      '/deploy?method=upload',
    )
  })

  it('connects through the popup, then lists repositories', async () => {
    configureMocks({ variant: 'deploy-github-disconnected' })
    const popup = { closed: false }
    const open = vi.spyOn(window, 'open').mockReturnValue(popup as Window)
    renderApp('/deploy?method=github')
    await userEvent.click(
      await screen.findByRole('button', { name: copy.github.connect }, { timeout: 5000 }),
    )
    expect(open).toHaveBeenCalledWith(
      expect.stringContaining('about:blank'),
      'openruntime-github',
      expect.any(String),
    )
    expect(await screen.findByText(copy.github.waiting)).toBeInTheDocument()
    // The user finishes consent in the popup.
    deployMockState().setGithubConnected(true)
    expect(
      await screen.findByText(copy.github.connectedAs('octocat'), {}, { timeout: 5000 }),
    ).toBeInTheDocument()
    expect((await repoRows()).length).toBeGreaterThan(2)
  })

  it('explains a blocked popup and offers a direct link', async () => {
    configureMocks({ variant: 'deploy-github-disconnected' })
    vi.spyOn(window, 'open').mockReturnValue(null)
    renderApp('/deploy?method=github')
    await userEvent.click(
      await screen.findByRole('button', { name: copy.github.connect }, { timeout: 5000 }),
    )
    expect(await screen.findByText(/Your browser blocked the GitHub window/)).toBeInTheDocument()
    expect(
      screen.getByRole('link', { name: new RegExp(copy.github.openHere) }).getAttribute('href'),
    ).toMatch(/^about:blank/)
  })

  it('never opens a login URL that is not https (review D2), and says connecting failed', async () => {
    configureMocks({ variant: 'deploy-github-disconnected' })
    server.use(
      http.get('*/api/github/login', () => HttpResponse.json({ auth_url: 'javascript:alert(1)' })),
    )
    const open = vi.spyOn(window, 'open')
    renderApp('/deploy?method=github')
    await userEvent.click(
      await screen.findByRole('button', { name: copy.github.connect }, { timeout: 5000 }),
    )
    expect(await screen.findByText(copy.github.connectFailed)).toBeInTheDocument()
    expect(open).not.toHaveBeenCalled()
  })

  it('searches repositories, with a way out of an empty search', async () => {
    renderApp('/deploy?method=github')
    await repoRows()
    await userEvent.type(screen.getByRole('searchbox', { name: copy.github.search }), 'fresh')
    expect((await repoRows()).map((r) => r.textContent)).toEqual([
      expect.stringContaining('acme/fresh-agent'),
    ])
    await userEvent.type(screen.getByRole('searchbox', { name: copy.github.search }), 'zzz')
    await userEvent.click(await screen.findByRole('button', { name: copy.github.clearSearch }))
    expect((await repoRows()).length).toBeGreaterThan(2)
  })

  it('says so when the account has no repositories', async () => {
    configureMocks({ variant: 'deploy-github-no-repos' })
    renderApp('/deploy?method=github')
    expect(
      await screen.findByText(copy.github.noRepos('octocat'), {}, { timeout: 5000 }),
    ).toBeInTheDocument()
  })

  it('picks a repo, pre-fills branch and name, clones and opens the build', async () => {
    const bodies = recordRequestBodies()
    const { router } = renderApp('/deploy?method=github')
    await pick('acme/fresh-agent')
    expect(router.state.location.search).toMatchObject({
      method: 'github',
      repo: 'acme/fresh-agent',
    })
    expect(screen.getByRole('textbox', { name: copy.github.branch })).toHaveValue('main')
    expect(screen.getByRole('textbox', { name: copy.deploy.name })).toHaveValue('fresh-agent')
    await userEvent.click(screen.getByRole('button', { name: 'Deploy acme/fresh-agent' }))
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/builds\//), {
      timeout: 5000,
    })
    bodies.stop()
    const sent = bodies.requests.find((r) => r.url.pathname === '/api/github/clone')!.body
    expect(sent).toEqual({
      repository_full_name: 'acme/fresh-agent',
      branch: 'main',
      agent_name: 'fresh-agent',
    })
    const id = router.state.location.pathname.split('/').pop()!
    // The server records no repository for a clone (D-12); this tab remembers it for the Build page's retry.
    expect(deployMockState().find(id)?.record.github_url).toBeNull()
    expect(clonedRepo(id)).toBe('acme/fresh-agent')
  })

  it('asks for a repository first, and rejects an invalid name before sending', async () => {
    renderApp('/deploy?method=github')
    await repoRows()
    await userEvent.click(screen.getByRole('button', { name: 'Deploy' }))
    expect(await screen.findByText(copy.github.pickFirst)).toBeInTheDocument()
    await pick('acme/fresh-agent')
    const name = screen.getByRole('textbox', { name: copy.deploy.name })
    await userEvent.clear(name)
    await userEvent.type(name, '-bad name')
    await userEvent.click(screen.getByRole('button', { name: 'Deploy acme/fresh-agent' }))
    expect(await screen.findByText(/Start with a letter/)).toBeInTheDocument()
  })

  it('turns a version clash into "Deploy as vX", which re-clones with version_override', async () => {
    const { router } = renderApp('/deploy?method=github')
    const rows = await repoRows()
    const existingRepo = rows
      .map((r) => r.textContent ?? '')
      .find((t) => /acme\/(?!fresh-agent|website)/.test(t))!
      .match(/acme\/[a-z0-9.-]+/)![0]
    await pick(existingRepo)
    await userEvent.click(screen.getByRole('button', { name: `Deploy ${existingRepo}` }))
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/builds\//), {
      timeout: 5000,
    })
    const id = router.state.location.pathname.split('/').pop()!
    // The worker clones and finds the version taken.
    deployMockState().advance(id, 'failed', 'failed')
    const again = await within(
      await screen.findByTestId('build-outcome', {}, { timeout: 5000 }),
    ).findByRole('link', { name: /^Deploy as v\d+\.\d+\.\d+$/ }, { timeout: 5000 })
    expect(again.getAttribute('href')).toMatch(/method=github/)
    expect(again.getAttribute('href')).toMatch(/version=\d+\.\d+\.\d+/)
    const bodies = recordRequestBodies()
    await userEvent.click(again)
    await userEvent.click(
      await screen.findByRole('button', { name: `Deploy ${existingRepo}` }, { timeout: 5000 }),
    )
    await waitFor(() => expect(router.state.location.pathname).not.toBe(`/builds/${id}`), {
      timeout: 5000,
    })
    bodies.stop()
    expect(bodies.requests.find((r) => r.url.pathname === '/api/github/clone')!.body).toMatchObject(
      { version_override: expect.stringMatching(/^\d+\.\d+\.\d+$/) },
    )
  })
})
