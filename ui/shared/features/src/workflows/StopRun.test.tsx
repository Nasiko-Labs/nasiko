/**
 * Stopping runs (nasiko-cloud-rs `feat/maf-stop-execution`): from a runs-list card, from the run page, and every
 * unfinished run of a workflow from its page. The seed's Contract intake has one paused run waiting for approval.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import { apiData, apiFetch } from '@nasiko/data-access/client'
import { configureMocks, workflowsMockState } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { recordRequestBodies } from '@nasiko/testing/setup'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'
import { execStatus } from './logic'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

const T = { timeout: 10_000 }
const contractId = () =>
  workflowsMockState().workflows.find((w) => w.name === 'Contract intake')!.id
/** The seed's paused run, and one that finished. */
async function seedRuns() {
  const runs = (await apiData<{ data: { id: string; status: string }[] }>('/api/maf/executions'))
    .data
  return {
    paused: runs.find((r) => r.status === 'awaiting_human')!.id,
    finished: runs.find((r) => r.status === 'success')!.id,
  }
}
const stopped = execStatus('stopped').label

async function confirmStop() {
  const dialog = await screen.findByRole('alertdialog', {}, T)
  await userEvent.click(within(dialog).getByRole('button', { name: copy.confirmStop }))
}

describe('Stopping workflow runs', () => {
  it('stops a paused run from its card on the runs list', async () => {
    const { paused } = await seedRuns()
    const bodies = recordRequestBodies()
    renderApp('/workflows/runs')
    const card = await waitFor(() => {
      const el = document.querySelector<HTMLElement>(`[data-card="${paused}"]`)
      expect(el).not.toBeNull()
      return el!
    }, T)
    await userEvent.click(within(card).getByRole('button', { name: copy.stopRun }))
    await confirmStop()
    expect(await screen.findByText(copy.runStopped, {}, T)).toBeInTheDocument()
    await waitFor(() => expect(within(card).getByText(stopped)).toBeInTheDocument(), T)
    expect(within(card).queryByRole('button', { name: copy.stopRun })).toBeNull()
    await bodies.flush()
    expect(
      bodies.requests.some(
        (r) => r.method === 'POST' && r.url.pathname === `/api/maf/execution/${paused}/stop`,
      ),
    ).toBe(true)
  })

  it('offers no Stop on the run page of a finished run', async () => {
    const { finished } = await seedRuns()
    renderApp(`/workflows/${contractId()}?run=${finished}`)
    // The run and its steps all read Complete.
    expect((await screen.findAllByText(execStatus('success').label, {}, T)).length).toBeGreaterThan(
      0,
    )
    expect(screen.queryByRole('button', { name: copy.stopRun })).toBeNull()
  })

  it('stops an unfinished run from its run page', async () => {
    const { paused } = await seedRuns()
    renderApp(`/workflows/${contractId()}?run=${paused}`)
    await userEvent.click(await screen.findByRole('button', { name: copy.stopRun }, T))
    await confirmStop()
    expect((await screen.findAllByText(stopped, {}, T)).length).toBeGreaterThan(0)
    await waitFor(() => expect(screen.queryByRole('button', { name: copy.stopRun })).toBeNull(), T)
  })

  it('says a run that finished meanwhile had already finished, without an error', async () => {
    const { paused } = await seedRuns()
    renderApp(`/workflows/${contractId()}?run=${paused}`)
    await userEvent.click(await screen.findByRole('button', { name: copy.stopRun }, T))
    // Stopped elsewhere (another tab, the CLI) between the click and the confirm.
    await apiFetch(`/api/maf/execution/${paused}/stop`, { method: 'POST' })
    await confirmStop()
    expect(await screen.findByText(copy.runAlreadyFinished, {}, T)).toBeInTheDocument()
    expect((await screen.findAllByText(stopped, {}, T)).length).toBeGreaterThan(0)
    expect(screen.queryByText(copy.stopFailed(''), { exact: false })).toBeNull()
  })

  it("stops every unfinished run of a workflow from the workflow's page", async () => {
    const bodies = recordRequestBodies()
    renderApp(`/workflows/${contractId()}`)
    await userEvent.click(await screen.findByRole('button', { name: copy.stopRuns(1) }, T))
    await confirmStop()
    expect(await screen.findByText(copy.runsStopped(1), {}, T)).toBeInTheDocument()
    await waitFor(
      () => expect(screen.queryByRole('button', { name: copy.stopRuns(1) })).toBeNull(),
      T,
    )
    await bodies.flush()
    const stop = bodies.requests.find(
      (r) => r.method === 'POST' && r.url.pathname === '/api/maf/executions/stop',
    )
    expect(stop?.body).toEqual({ workflow_id: contractId() })
  })
})
