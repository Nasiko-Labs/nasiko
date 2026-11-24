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

  it('goes from a harness to its sessions, a dataset, and a task × model table', async () => {
    renderApp('/evals')
    const harness = await screen.findByRole('link', { name: /Claude Code/ }, T)
    expect(harness).toHaveAttribute('href', '/evals/claude')
    await userEvent.click(harness)

    const generate = await screen.findByRole('button', { name: copy.generate(0) }, T)
    expect(generate).toBeDisabled()
    const [first] = await screen.findAllByRole('checkbox', { name: /^Select (?!all)/ }, T)
    await userEvent.click(first!)
    await userEvent.click(screen.getByRole('button', { name: copy.generate(1) }))

    const dataset = await screen.findByRole('region', { name: copy.dataset.title }, T)
    // Tasks start ticked, and the models the session used.
    const models = within(dataset)
      .getAllByRole('checkbox')
      .filter((c) => c.id.startsWith('model-'))
    const used = models.filter((c) => c.getAttribute('aria-checked') === 'true')
    expect(used.length).toBeGreaterThan(0)
    const extra = models.find((c) => c.getAttribute('aria-checked') !== 'true')!
    await userEvent.click(extra)
    await userEvent.click(within(dataset).getByRole('button', { name: copy.run }))

    const results = await screen.findByRole('region', { name: copy.results.title }, T)
    const table = within(results).getByRole('table')
    // A task column plus one per picked model.
    expect(within(table).getAllByRole('columnheader')).toHaveLength(used.length + 2)
    expect(within(table).getAllByText(/\d+\/100/).length).toBeGreaterThan(0)
    expect(
      within(table).getAllByText(`${copy.results.best}:`, { exact: false }).length,
    ).toBeGreaterThan(0)

    // Ticking another session sets the dataset aside until Regenerate.
    const [, second] = screen.getAllByRole('checkbox', { name: /^Select (?!all)/ })
    await userEvent.click(second!)
    await waitFor(() =>
      expect(screen.queryByRole('region', { name: copy.dataset.title })).toBeNull(),
    )
    expect(screen.getByRole('button', { name: copy.regenerate(2) })).toBeEnabled()
  })

  it('says when the server has no evals routes', async () => {
    configureMocks({ variant: 'evals-absent' })
    renderApp('/evals')
    expect(await screen.findByText(copy.absent, {}, T)).toBeInTheDocument()
  })
})
