import { screen, waitFor, within } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, deployMockState } from '@/mocks/handlers'
import { buildId, ORPHAN_ID } from '@/mocks/deploy'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequests, server } from '@/test/setup'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null }))

const steps = async () => {
  const list = await screen.findByRole('list', { name: copy.build.stepsLabel }, { timeout: 5000 })
  return within(list)
    .getAllByRole('listitem')
    .map((li) => li.dataset.state)
}

describe('Build page (plans/feat-deploy.md §5)', () => {
  it('follows a running build live to "is running" with Chat and Open agent', async () => {
    renderApp(`/builds/${buildId(1)}`)
    await waitFor(
      async () => expect(await steps()).toEqual(['done', 'current', 'pending', 'pending']),
      { timeout: 5000 },
    )
    expect(screen.getByText(copy.build.leave)).toBeInTheDocument()
    expect(
      screen
        .getByRole('list', { name: copy.build.stepsLabel })
        .querySelector('[aria-current="step"]'),
    ).not.toBeNull()
    // The server's worker finishes the image, then the deploy: the stream sends success and closes.
    deployMockState().advance(buildId(1), 'success', 'completed')
    const outcome = await screen.findByTestId('build-outcome', {}, { timeout: 5000 })
    expect(outcome.dataset.outcome).toBe('running')
    expect(
      within(outcome).getByRole('link', { name: copy.build.chat }).getAttribute('href'),
    ).toMatch(/^\/chat\?/)
    expect(
      within(outcome).getByRole('link', { name: copy.build.openAgent }).getAttribute('href'),
    ).toMatch(/^\/agents\//)
    expect(await steps()).toEqual(['done', 'done', 'done', 'done'])
    expect(screen.queryByText(copy.build.leave)).toBeNull()
  })

  it('re-reads a stale "down" agent once the image is built (a redeploy of a crashed agent), never a false "not running"', async () => {
    const agentId = deployMockState().find(buildId(1))!.record.agent_id
    deployMockState().setAgentStatus(agentId, 'crashed')
    renderApp(`/builds/${buildId(1)}`)
    await waitFor(
      async () => expect(await steps()).toEqual(['done', 'current', 'pending', 'pending']),
      { timeout: 5000 },
    )
    // The redeploy brings it back; the page's cached directory still says crashed.
    deployMockState().setAgentStatus(agentId, 'running')
    deployMockState().advance(buildId(1), 'success', 'completed')
    await waitFor(
      () => expect(screen.getByTestId('build-outcome').dataset.outcome).toBe('running'),
      { timeout: 8000 },
    )
  }, 12_000)

  it('shows a failed build with the failing stage, the problem and the fix', async () => {
    renderApp(`/builds/${buildId(3)}`)
    const outcome = await screen.findByTestId('build-outcome', {}, { timeout: 5000 })
    expect(outcome.dataset.outcome).toBe('failed')
    expect(within(outcome).getByText(copy.errors.noDockerfile.problem)).toBeInTheDocument()
    expect(within(outcome).getByText(copy.errors.noDockerfile.fix)).toBeInTheDocument()
    expect(await steps()).toEqual(['done', 'failed', 'pending', 'pending'])
  })

  it('shows a failed first upload from its upload row when the stream says not_found (D-4)', async () => {
    renderApp(`/builds/${ORPHAN_ID}`)
    const outcome = await screen.findByTestId('build-outcome', {}, { timeout: 5000 })
    expect(outcome.dataset.outcome).toBe('failed')
    expect(within(outcome).getByText(copy.errors.noEntrypoint.problem)).toBeInTheDocument()
  })

  it('falls back to polling the upload row when the stream drops', async () => {
    server.use(
      http.get(
        '*/api/agents/deploys/:id/stream',
        () => new HttpResponse('upstream reset', { status: 502 }),
      ),
    )
    renderApp(`/builds/${buildId(1)}`)
    await waitFor(
      async () => expect(await steps()).toEqual(['done', 'current', 'pending', 'pending']),
      { timeout: 5000 },
    )
    deployMockState().advance(buildId(1), 'success', 'completed')
    const outcome = await screen.findByTestId('build-outcome', {}, { timeout: 8000 })
    expect(outcome.dataset.outcome).toBe('running')
  }, 12_000)

  it('says "Built, but not running" when the agent went down after the image built', async () => {
    const b = deployMockState().find(buildId(1))!
    deployMockState().advance(buildId(1), 'success', 'orchestration_processing')
    deployMockState().setAgentStatus(b.record.agent_id, 'failed')
    renderApp(`/builds/${buildId(1)}`)
    const outcome = await screen.findByTestId('build-outcome', {}, { timeout: 5000 })
    expect(outcome.dataset.outcome).toBe('notRunning')
    expect(within(outcome).getByText(copy.build.notRunning)).toBeInTheDocument()
    expect((await steps()).at(-1)).toBe('warning')
  })

  it('says not found for an unknown build, and stops asking', async () => {
    const rec = recordRequests()
    renderApp('/builds/5eed000b-0000-4000-8000-000000000404')
    expect(await screen.findByText(copy.build.notFound, {}, { timeout: 5000 })).toBeInTheDocument()
    const reads = () =>
      rec.urls.filter((u) => u.pathname.endsWith('/uploads/5eed000b-0000-4000-8000-000000000404'))
        .length
    const before = reads()
    await new Promise((r) => setTimeout(r, 3500))
    expect(reads()).toBe(before)
    rec.stop()
    expect(screen.getByRole('link', { name: copy.build.allBuilds }).getAttribute('href')).toBe(
      '/builds',
    )
  })

  it('says no rights when the build reads answer {available:false}', async () => {
    configureMocks({ variant: 'deploy-no-rights' })
    renderApp(`/builds/${buildId(1)}`)
    expect(await screen.findByText(copy.builds.noRights, {}, { timeout: 5000 })).toBeInTheDocument()
  })
})
