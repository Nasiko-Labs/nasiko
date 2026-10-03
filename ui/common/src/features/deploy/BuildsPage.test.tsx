import { cleanup, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, deployMockState } from '@/mocks/handlers'
import { buildId, DEMO } from '@/mocks/deploy'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const rows = async () => {
  const table = await screen.findByTestId('builds-table', {}, { timeout: 5000 })
  return within(table).getAllByTestId('build-row')
}

describe('Builds (plans/feat-deploy.md §6)', () => {
  it('pins in-progress builds on top, then newest first', async () => {
    renderApp('/builds')
    const r = await rows()
    // The mock's live demo builds: one building, one queued (design review 2).
    expect(
      r
        .slice(0, 2)
        .map((x) => x.dataset.status)
        .sort(),
    ).toEqual(['building', 'queued'])
    expect(
      r.slice(2).every((x) => x.dataset.status !== 'building' && x.dataset.status !== 'queued'),
    ).toBe(true)
    expect(within(r[0]!).getByRole('link').getAttribute('href')).toMatch(/^\/builds\/5eed/)
  })

  it('gives failed rows a one-line reason, never raw server text', async () => {
    renderApp('/builds?status=failed')
    await waitFor(
      async () => {
        const reasons = (await rows()).map((x) => within(x).getByTestId('build-reason').textContent)
        expect(reasons).toContain(copy.errors.noDockerfile.problem)
        expect(reasons.some((t) => /^Version \d+\.\d+\.\d+ already exists\.$/.test(t ?? ''))).toBe(
          true,
        )
      },
      { timeout: 5000 },
    )
    expect(screen.queryByText(/no Dockerfile found in root of zip/)).toBeNull()
  })

  it('filters by status without pinning, and searches by agent or version', async () => {
    renderApp('/builds?status=success')
    expect((await rows()).every((x) => x.dataset.status === 'success')).toBe(true)
    await userEvent.type(
      screen.getByRole('searchbox', { name: copy.builds.searchLabel }),
      'no-such-agent',
    )
    expect(await screen.findByText(copy.builds.noMatch, {}, { timeout: 5000 })).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: copy.builds.clearFilters }))
    expect((await rows()).length).toBeGreaterThan(2)
  })

  it('shows a non-superuser only the builds of agents they own', async () => {
    configureMocks({ superuser: false })
    renderApp('/builds')
    const mine = new Set(
      deployMockState()
        .state.builds.filter((b) => b.ownerId === deployMockState().state.builds[0]!.ownerId)
        .map((b) => b.record.id),
    )
    const hrefs = (await rows()).map((x) =>
      within(x).getByRole('link').getAttribute('href')!.split('/').pop()!,
    )
    expect(hrefs.every((id) => mine.has(id))).toBe(true)
  })

  it('refreshes while a build runs and shows it finishing', async () => {
    renderApp('/builds')
    await rows()
    deployMockState().advance(buildId(1), 'success', 'completed')
    await waitFor(
      async () => {
        const all = await rows()
        expect(all.filter((x) => x.dataset.status === 'building')).toHaveLength(0)
        // It moves from the pinned rows into the list, never off the page (review: list re-read when a pin leaves).
        expect(
          all.some(
            (x) =>
              within(x).getByRole('link').getAttribute('href') === `/builds/${buildId(1)}` &&
              x.dataset.status === 'success',
          ),
        ).toBe(true)
      },
      { timeout: 8000 },
    )
  }, 12_000)

  it('says when the server has no builds route, and when the caller has no deploy rights', async () => {
    configureMocks({ variant: 'builds-absent' })
    renderApp('/builds')
    expect(
      await screen.findByText(copy.builds.newerServer, {}, { timeout: 5000 }),
    ).toBeInTheDocument()
    cleanup()
    configureMocks({ variant: 'deploy-no-rights' })
    renderApp('/builds')
    expect(await screen.findByText(copy.builds.noRights, {}, { timeout: 5000 })).toBeInTheDocument()
  })

  it('links the demo building build to its page', async () => {
    renderApp('/builds')
    const r = await rows()
    const hrefs = r.map((x) => within(x).getByRole('link').getAttribute('href'))
    expect(hrefs).toContain(`/builds/${buildId(1)}`)
    expect(DEMO.building).toBe(1)
  })
})
