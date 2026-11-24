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

  it('walks the stepper from sessions: sessions, dataset, then a task × model comparison', async () => {
    renderApp('/evals')
    // Sessions: Generate waits for a tick; the Dataset step for a dataset.
    const generate = await screen.findByRole('button', { name: copy.generate(0) }, T)
    expect(generate).toBeDisabled()
    expect(stepButton(copy.steps.sessions)).toHaveAttribute('aria-current', 'step')
    expect(stepButton(copy.steps.dataset)).toBeDisabled()
    const [first] = await sessionBoxes()
    await userEvent.click(first!)
    await userEvent.click(screen.getByRole('button', { name: copy.generate(1) }))

    // Dataset: the step changes in place and its content takes focus.
    const dataset = await screen.findByRole('region', { name: copy.steps.dataset }, T)
    await waitFor(() => expect(dataset).toHaveFocus())
    expect(stepButton(copy.steps.dataset)).toHaveAttribute('aria-current', 'step')
    // Tasks start ticked, and the models the session used.
    const models = within(dataset)
      .getAllByRole('checkbox')
      .filter((c) => c.id.startsWith('model-'))
    const used = ticked(models)
    expect(used.length).toBeGreaterThan(0)
    await userEvent.click(models.find((c) => !used.includes(c))!)
    await userEvent.click(screen.getByRole('button', { name: copy.run }))

    // Compare: a column per picked model, the row's best starred, and a scoreboard naming the best overall.
    const results = await screen.findByRole('region', { name: copy.steps.compare }, T)
    await waitFor(() => expect(results).toHaveFocus())
    const table = within(results).getByRole('table')
    expect(within(table).getAllByRole('columnheader')).toHaveLength(used.length + 2)
    expect(within(table).getAllByText(/\d+\/100/).length).toBeGreaterThan(0)
    expect(
      within(table).getAllByText(`${copy.results.best}:`, { exact: false }).length,
    ).toBeGreaterThan(0)
    expect(within(results).getAllByText(copy.results.bestOverall).length).toBeGreaterThan(0)

    // Back on Sessions, ticking another session sets the dataset aside until Regenerate.
    await userEvent.click(stepButton(copy.steps.sessions))
    const [, second] = await sessionBoxes()
    await userEvent.click(second!)
    expect(stepButton(copy.steps.dataset)).toBeDisabled()
    expect(screen.getByRole('button', { name: copy.regenerate(2) })).toBeEnabled()
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
    const results = await screen.findByRole('region', { name: copy.steps.compare }, T)
    expect(within(results).getAllByRole('columnheader')).toHaveLength(2)
  })

  it('says when the server has no evals routes', async () => {
    configureMocks({ variant: 'evals-absent' })
    renderApp('/evals')
    expect(await screen.findByText(copy.absent, {}, T)).toBeInTheDocument()
  })
})
