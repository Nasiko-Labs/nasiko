import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

const T = { timeout: 8000 }
/** A step's circle on the stepper's track. */
const stepButton = (title: string) => screen.getByRole('button', { name: `${title} step` })
const sessionBoxes = () => screen.findAllByRole('checkbox', { name: /^Select (?!all)/ }, T)
const usable = (boxes: HTMLElement[]) => boxes.filter((b) => !b.hasAttribute('data-disabled'))
const ticked = (boxes: HTMLElement[]) =>
  boxes.filter((c) => c.getAttribute('aria-checked') === 'true')

describe('Evals (docs/superpowers/specs/2026-11-23-evals-design.md)', () => {
  it('with Claude Code not connected, says so and links to Coding harnesses', async () => {
    configureMocks({ variant: 'no-harnesses' })
    renderApp('/evals')
    expect(await screen.findByText(copy.empty.title, {}, T)).toBeInTheDocument()
    expect(screen.getByRole('link', { name: copy.empty.action })).toHaveAttribute(
      'href',
      '/harnesses',
    )
    expect(screen.queryByRole('navigation', { name: copy.steps.label })).toBeNull()
  })

  it('walks the stepper from sessions to a task × model comparison, and keeps the run', async () => {
    renderApp('/evals')
    // Sessions: Generate waits for a tick; the Dataset step for a dataset.
    const generate = await screen.findByRole('button', { name: copy.generate(0) }, T)
    expect(generate).toBeDisabled()
    expect(stepButton(copy.steps.sessions)).toHaveAttribute('aria-current', 'step')
    expect(stepButton(copy.steps.dataset)).toBeDisabled()
    // Sessions that can't become a task can't be ticked, and say why.
    const boxes = await sessionBoxes()
    const open = usable(boxes)
    expect(open.length).toBeGreaterThan(0)
    if (open.length < boxes.length)
      expect(screen.getAllByText(copy.sessions.ineligible.no_repo!).length).toBeGreaterThan(0)
    await userEvent.click(open[0]!)
    await userEvent.click(screen.getByRole('button', { name: copy.generate(1) }))

    // Dataset: the step changes in place and its content takes focus; the dataset is built in the background, then
    // its verified tasks and the session's models start ticked.
    const dataset = await screen.findByRole('region', { name: copy.steps.dataset }, T)
    await waitFor(() => expect(dataset).toHaveFocus())
    expect(stepButton(copy.steps.dataset)).toHaveAttribute('aria-current', 'step')
    const run = screen.getByRole('button', { name: copy.run })
    await waitFor(() => expect(run).toBeEnabled(), T)
    const models = within(dataset)
      .getAllByRole('checkbox')
      .filter((c) => c.id.startsWith('model-'))
    const used = ticked(models)
    expect(used.length).toBeGreaterThan(0)
    await userEvent.click(run)

    // Compare: the task, the original session, a column per picked model; the row's best starred, and a scoreboard
    // naming the best overall once the run ends.
    const results = await screen.findByRole('region', { name: copy.steps.compare }, T)
    await waitFor(() => expect(results).toHaveFocus())
    const table = within(results).getByRole('table')
    expect(within(table).getAllByRole('columnheader')).toHaveLength(used.length + 2)
    expect(await within(table).findAllByText(/\d+\/100/, {}, T)).not.toHaveLength(0)
    expect(
      within(table).getAllByText(`${copy.results.best}:`, { exact: false }).length,
    ).toBeGreaterThan(0)
    expect(within(results).getAllByText(copy.results.bestOverall).length).toBeGreaterThan(0)

    // Back on Sessions, the run is saved, and reopens from the URL.
    await userEvent.click(stepButton(copy.steps.sessions))
    const saved = await screen.findByRole('link', { name: /Open the run of/ }, T)
    expect(saved.getAttribute('href')).toMatch(/^\/evals\?dataset=.+&run=.+/)
  })

  it('asks to connect GitHub when it is not, and offers no session', async () => {
    configureMocks({ variant: 'evals-github-off' })
    renderApp('/evals')
    expect(await screen.findByText(copy.github.title, {}, T)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: copy.github.action })).toBeEnabled()
    expect(usable(await sessionBoxes())).toHaveLength(0)
  })

  it('with no sessions, starts from benchmarks: several picked, their tasks combined', async () => {
    configureMocks({ variant: 'no-activity' })
    renderApp('/evals')
    expect(
      await screen.findByRole('heading', { name: copy.benchmarks.title }, T),
    ).toBeInTheDocument()
    expect(stepButton(copy.steps.benchmarks)).toHaveAttribute('aria-current', 'step')
    expect(screen.queryByRole('button', { name: `${copy.steps.sessions} step` })).toBeNull()

    const go = screen.getByRole('button', { name: copy.goToDataset })
    expect(go).toBeDisabled()
    await userEvent.click(await screen.findByRole('checkbox', { name: 'SWE-bench Verified' }, T))
    await userEvent.click(screen.getByRole('checkbox', { name: 'Terminal-Bench' }))
    await userEvent.click(go)

    // Both benchmarks' tasks, all ticked; no model was used by a session, so none is ticked yet.
    const dataset = await screen.findByRole('region', { name: copy.steps.dataset }, T)
    const boxes = within(dataset).getAllByRole('checkbox')
    const tasks = boxes.filter((c) => c.id.startsWith('task-'))
    expect(tasks.some((c) => c.id.includes('swe-bench-verified'))).toBe(true)
    expect(tasks.some((c) => c.id.includes('terminal-bench'))).toBe(true)
    expect(ticked(tasks)).toHaveLength(tasks.length)
    expect(ticked(boxes.filter((c) => c.id.startsWith('model-')))).toHaveLength(0)
    expect(screen.getByRole('button', { name: copy.run })).toBeDisabled()

    await userEvent.click(within(dataset).getByRole('checkbox', { name: 'GPT-5' }))
    await userEvent.click(screen.getByRole('button', { name: copy.run }))
    // No original session to compare with: the task and the one model.
    const results = await screen.findByRole('region', { name: copy.steps.compare }, T)
    expect(within(results).getAllByRole('columnheader')).toHaveLength(2)
  })

  it('says when the server has no evals routes', async () => {
    configureMocks({ variant: 'evals-absent' })
    renderApp('/evals')
    expect(await screen.findByText(copy.absent, {}, T)).toBeInTheDocument()
  })
})
