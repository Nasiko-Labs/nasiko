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

describe('Evals (docs/superpowers/specs/2026-11-23-evals-design.md)', () => {
  it('with no coding harness connected, says so and links to Coding harnesses', async () => {
    configureMocks({ variant: 'no-harnesses' })
    renderApp('/evals')
    expect(await screen.findByText(copy.empty.title, {}, T)).toBeInTheDocument()
    expect(screen.getByRole('link', { name: copy.empty.action })).toHaveAttribute(
      'href',
      '/harnesses',
    )
  })

  it('goes from a harness to its sessions, a dataset, and a task × model table, then keeps the run', async () => {
    renderApp('/evals')
    const harness = await screen.findByRole('link', { name: /Claude Code/ }, T)
    expect(harness).toHaveAttribute('href', '/evals/claude')
    await userEvent.click(harness)

    const generate = await screen.findByRole('button', { name: copy.generate(0) }, T)
    expect(generate).toBeDisabled()
    // Sessions that can't become a task can't be ticked, and say why.
    const boxes = await screen.findAllByRole('checkbox', { name: /^Select (?!all)/ }, T)
    const usable = boxes.filter((b) => !b.hasAttribute('data-disabled'))
    expect(usable.length).toBeGreaterThan(0)
    if (usable.length < boxes.length)
      expect(screen.getAllByText(copy.sessions.ineligible.no_repo!).length).toBeGreaterThan(0)
    await userEvent.click(usable[0]!)
    await userEvent.click(screen.getByRole('button', { name: copy.generate(1) }))

    const dataset = await screen.findByRole('region', { name: copy.dataset.title }, T)
    await waitFor(() =>
      expect(screen.getByRole('heading', { name: copy.dataset.title })).toHaveFocus(),
    )
    // The dataset is built in the background, then its verified tasks and the session's models start ticked.
    const run = await within(dataset).findByRole('button', { name: copy.run }, T)
    const models = within(dataset)
      .getAllByRole('checkbox')
      .filter((c) => c.id.startsWith('model-'))
    const used = models.filter((c) => c.getAttribute('aria-checked') === 'true')
    expect(used.length).toBeGreaterThan(0)
    await userEvent.click(run)

    const results = await screen.findByRole('region', { name: copy.results.title }, T)
    await waitFor(() =>
      expect(screen.getByRole('heading', { name: copy.results.title })).toHaveFocus(),
    )
    const table = within(results).getByRole('table')
    // A task column, the original session's, then one per picked model.
    expect(within(table).getAllByRole('columnheader')).toHaveLength(used.length + 2)
    expect(await within(table).findAllByText(/\d+\/100/, {}, T)).not.toHaveLength(0)
    expect(
      within(table).getAllByText(`${copy.results.best}:`, { exact: false }).length,
    ).toBeGreaterThan(0)

    // The run is saved: /evals lists it, and it reopens on its harness's page.
    renderApp('/evals')
    const saved = await screen.findByRole('link', { name: /Open the run of/ }, T)
    expect(saved.getAttribute('href')).toMatch(/^\/evals\/claude\?dataset=.+&run=.+/)
  })

  it('asks to connect GitHub when it is not, and offers no session', async () => {
    configureMocks({ variant: 'evals-github-off' })
    renderApp('/evals/claude')
    expect(await screen.findByText(copy.github.title, {}, T)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: copy.github.action })).toBeEnabled()
    const boxes = await screen.findAllByRole('checkbox', { name: /^Select (?!all)/ }, T)
    expect(boxes.every((b) => b.hasAttribute('data-disabled'))).toBe(true)
  })

  it('says when the server has no evals routes', async () => {
    configureMocks({ variant: 'evals-absent' })
    renderApp('/evals')
    expect(await screen.findByText(copy.absent, {}, T)).toBeInTheDocument()
  })
})
