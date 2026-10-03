import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, workflowsMockState } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { recordRequestBodies } from '@/test/setup'
import { renderApp } from '@/test/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

const T = { timeout: 10_000 }
const idOf = (name: string) => workflowsMockState().workflows.find((w) => w.name === name)!.id
/** The run that Run landed on (`?run=`): its card, open, titled `title`. */
async function openRunCard(router: ReturnType<typeof renderApp>['router'], title: RegExp) {
  await waitFor(() => expect(router.state.location.search).toHaveProperty('run'), T)
  const id = (router.state.location.search as { run: string }).run
  const card = await waitFor(() => {
    const el = document.querySelector<HTMLElement>(`[data-card="${id}"]`)
    expect(el).toHaveAttribute('data-state', 'open')
    return el!
  }, T)
  expect(within(card).getByRole('button', { name: title })).toBeInTheDocument()
  return card
}

/** A clock the test moves (configureMocks({ now }) rebuilds the state, so set it once). */
function clock() {
  let t = now()
  configureMocks({ now: () => t })
  return { advance: (ms: number) => (t += ms) }
}

describe('Create workflow (plans/feat-workflows.md §3, §4)', () => {
  it('deploys a new workflow with typed steps, then runs it from the dialog', async () => {
    const c = clock()
    const bodies = recordRequestBodies()
    const { router } = renderApp('/workflows/new')
    const deploy = await screen.findByRole('button', { name: copy.deploy }, T)
    expect(deploy).toBeDisabled()
    await userEvent.type(screen.getByRole('textbox', { name: copy.nameRequired }), 'Weekly digest')
    await userEvent.type(screen.getByLabelText(copy.instructions(1)), 'Pull last week revenue')
    await userEvent.click(screen.getByRole('button', { name: copy.addStep }))
    // The new step's instructions get focus.
    await waitFor(() => expect(screen.getByLabelText(copy.instructions(2))).toHaveFocus())
    await userEvent.keyboard('Write the digest')
    await userEvent.type(screen.getByLabelText(copy.agentFor(2)), 'Support Bot')
    await userEvent.click(deploy)
    const dialog = await screen.findByRole('alertdialog', { name: copy.deployedTitle }, T)
    await bodies.flush()
    const create = bodies.requests.find(
      (r) => r.method === 'POST' && r.url.pathname === '/api/maf/workflows',
    )!
    expect(create.body).toMatchObject({
      name: 'Weekly digest',
      steps: [
        { step_index: 0, task_description: 'Pull last week revenue' },
        { step_index: 1, task_description: 'Write the digest', agent_id: seed.agents[0]!.id },
      ],
    })
    expect((create.body as { steps: object[] }).steps[0]).not.toHaveProperty('agent_id')
    await userEvent.click(within(dialog).getByRole('button', { name: copy.runWorkflow }))
    // The Runs tab, with the new run open.
    await waitFor(() => expect(router.state.location.pathname).toBe('/workflows/runs'))
    const run = await openRunCard(router, /^Weekly digest #\d+$/)
    c.advance(60_000)
    await waitFor(() => expect(within(run).getAllByText('Complete').length).toBeGreaterThan(1), T)
  })

  it('drafts steps with the planner, and asks before replacing edited steps', async () => {
    renderApp('/workflows/new')
    const desc = await screen.findByRole('textbox', { name: copy.descLabel }, T)
    await userEvent.type(desc, 'Pull revenue by region, then write the brief')
    await userEvent.click(screen.getByRole('button', { name: copy.generate }))
    expect(await screen.findByDisplayValue('Pull revenue by region', {}, T)).toBeInTheDocument()
    expect(screen.getByDisplayValue('Write the brief')).toBeInTheDocument()
    expect(screen.getAllByText(copy.suggested)).toHaveLength(2)
    // The planner named it, since no name was typed.
    expect(screen.getByLabelText(copy.nameLabel)).toHaveValue('Pull revenue by region, then')
    // Same text: nothing to regenerate. Edit a step, change the text: replacing asks first.
    expect(screen.getByRole('button', { name: copy.regenerate })).toBeDisabled()
    await userEvent.type(screen.getByLabelText(copy.instructions(1)), ' fast')
    await userEvent.type(desc, '.')
    await userEvent.click(screen.getByRole('button', { name: copy.regenerate }))
    const ask = await screen.findByRole('alertdialog', { name: copy.replaceTitle })
    await userEvent.click(within(ask).getByRole('button', { name: copy.keepSteps }))
    expect(screen.getByDisplayValue('Pull revenue by region fast')).toBeInTheDocument()
  })

  it('says why the planner refused', async () => {
    configureMocks({ variant: 'workflows-no-key' })
    renderApp('/workflows/new')
    await userEvent.type(
      await screen.findByRole('textbox', { name: copy.descLabel }, T),
      'Anything',
    )
    await userEvent.click(screen.getByRole('button', { name: copy.generate }))
    expect(await screen.findByText(copy.plan['no-key'], {}, T)).toBeInTheDocument()
  })

  it('moves a step with the arrow keys on its grip, and says where it landed', async () => {
    renderApp('/workflows/new')
    await userEvent.type(await screen.findByLabelText(copy.instructions(1), {}, T), 'First')
    await userEvent.click(screen.getByRole('button', { name: copy.addStep }))
    await userEvent.keyboard('Second')
    screen.getByRole('button', { name: copy.reorder(2) }).focus()
    await userEvent.keyboard('{ArrowUp}')
    expect(screen.getByLabelText(copy.instructions(1))).toHaveValue('Second')
    expect(screen.getByText(copy.moved(1, 2))).toBeInTheDocument()
    await waitFor(() => expect(screen.getByRole('button', { name: copy.reorder(1) })).toHaveFocus())
  })

  it('saves a draft and opens it; leaving with changes asks first', async () => {
    const { router } = renderApp('/workflows/new')
    await userEvent.type(
      await screen.findByRole('textbox', { name: copy.nameRequired }, T),
      'Lead scoring',
    )
    await userEvent.click(screen.getByRole('link', { name: copy.back }))
    const leave = await screen.findByRole('alertdialog', { name: copy.leaveTitle })
    await userEvent.click(within(leave).getByRole('button', { name: copy.keepEditing }))
    await userEvent.type(screen.getByLabelText(copy.instructions(1)), 'Score each new lead')
    await userEvent.click(screen.getByRole('button', { name: copy.saveDraft }))
    await waitFor(() =>
      expect(router.state.location.pathname).toBe(`/workflows/${idOf('Lead scoring')}`),
    )
    const wf = workflowsMockState().workflows.find((w) => w.name === 'Lead scoring')!
    expect(wf).toMatchObject({ status: 'draft', drafted: true })
    expect(wf.steps.map((s) => s.task_description)).toEqual(['Score each new lead'])
    // A draft opens in the editor.
    expect(await screen.findByRole('button', { name: copy.deploy }, T)).toBeInTheDocument()
  })

  it('on `main` there is no draft button (W-1)', async () => {
    configureMocks({ variant: 'workflows-classic' })
    renderApp('/workflows/new')
    expect(await screen.findByRole('button', { name: copy.deploy }, T)).toBeInTheDocument()
    await waitFor(() => expect(screen.queryByRole('button', { name: copy.saveDraft })).toBeNull())
  })
})

describe('Workflow page and runs (plans/feat-workflows.md §5, §6)', () => {
  it('shows a live workflow read-only, and edits it behind Edit', async () => {
    renderApp(`/workflows/${idOf('Quarterly report')}`)
    expect(
      await screen.findByRole('heading', { name: 'Quarterly report', level: 1 }, T),
    ).toBeVisible()
    expect(screen.getByText('Pull revenue and churn by region for the quarter')).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: copy.outputGuidelines })).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: copy.edit }))
    expect(await screen.findByLabelText(copy.nameLabel)).toHaveValue('Quarterly report')
    await userEvent.click(screen.getByRole('button', { name: copy.cancel }))
    expect(await screen.findByRole('button', { name: copy.edit })).toBeInTheDocument()
  })

  it('saves and runs a draft, then Back returns to the draft', async () => {
    const c = clock()
    const { router } = renderApp(`/workflows/${idOf('Onboarding checklist')}`)
    await userEvent.click(await screen.findByRole('button', { name: copy.saveAndRun }, T))
    await waitFor(() => expect(router.state.location.pathname).toBe('/workflows/runs'))
    const run = await openRunCard(router, /^Onboarding checklist #\d+$/)
    c.advance(60_000)
    await waitFor(() => expect(within(run).getAllByText('Complete').length).toBeGreaterThan(1), T)
    router.history.back()
    await waitFor(() =>
      expect(router.state.location.pathname).toBe(`/workflows/${idOf('Onboarding checklist')}`),
    )
    expect(await screen.findByRole('button', { name: copy.deploy }, T)).toBeInTheDocument()
  })

  it('answers a paused run in place, and the run finishes', async () => {
    const c = clock()
    // The seeded paused run raises its request when first read.
    renderApp('/workflows/runs')
    const titles = await screen.findAllByRole('button', { name: /Contract intake #\d+/ }, T)
    const title = titles.find((b) =>
      within(b.closest('[data-card]') as HTMLElement).queryByText('Awaiting action'),
    )!
    const cardEl = title.closest('[data-card]') as HTMLElement
    await userEvent.click(title)
    const request = await within(cardEl).findByTestId('request-card', {}, T)
    // A single-choice option answers on click.
    await userEvent.click(within(request).getByRole('button', { name: /^Approve/ }))
    await waitFor(() => expect(within(cardEl).queryByTestId('request-card')).toBeNull(), T)
    c.advance(60_000)
    await waitFor(() => expect(within(cardEl).queryByText('Awaiting action')).toBeNull(), T)
    expect(within(cardEl).getByTestId('request-receipt')).toBeInTheDocument()
  })

  it('lists every run, filters them, and marks a deleted workflow', async () => {
    renderApp('/workflows/runs')
    const orphan = await screen.findByRole('button', { name: /Legacy export #\d+/ }, T)
    expect(
      within(orphan.closest('[data-card]') as HTMLElement).getByText(copy.orphan),
    ).toBeInTheDocument()
    await userEvent.click(screen.getByRole('combobox', { name: copy.statusFilter }))
    await userEvent.click(await screen.findByRole('option', { name: 'Failed' }))
    await waitFor(() => expect(screen.queryByRole('button', { name: /Legacy export/ })).toBeNull())
    const failed = screen.getAllByRole('button', { name: /#\d+$/ })
    expect(failed.length).toBe(3)
    const rerun = within(failed[0]!.closest('[data-card]') as HTMLElement).getByRole('button', {
      name: copy.rerun,
    })
    expect(rerun).toBeEnabled()
  })

  it('runs filtered to nothing say so, and Clear filters resets every filter', async () => {
    const { router } = renderApp('/workflows/runs?q=zzz-no-such-run&status=failed')
    expect(await screen.findByText(copy.noRunsMatch, {}, T)).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: copy.clearFilters }))
    await waitFor(() => expect(router.state.location.search).toEqual({}))
    expect(await screen.findByRole('button', { name: /Legacy export #\d+/ }, T)).toBeVisible()
  })

  it('a missing workflow is one dead end', async () => {
    renderApp('/workflows/5eed0010-0000-4000-8000-00000000ffff')
    expect(await screen.findByRole('heading', { name: copy.notFound }, T)).toBeVisible()
    expect(screen.getByRole('link', { name: copy.backToWorkflows })).toHaveAttribute(
      'href',
      '/workflows',
    )
  })
})
