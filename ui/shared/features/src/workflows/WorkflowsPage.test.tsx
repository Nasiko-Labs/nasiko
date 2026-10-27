import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { recordRequests } from '@nasiko/testing/setup'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

const T = { timeout: 8000 }
const card = async (name: string) =>
  (await screen.findByRole('link', { name }, T)).closest('[data-slot="card"]') as HTMLElement

describe('Workflow lists (plans/feat-workflows.md §2)', () => {
  it('shows deployed workflows with their metrics, health and last run', async () => {
    renderApp('/workflows')
    expect(
      await screen.findByRole('link', { name: copy.nav.deployed, current: 'page' }, T),
    ).toBeVisible()
    const ticket = await card('Ticket resolution')
    expect(within(ticket).getByText('12 runs')).toBeInTheDocument()
    expect(within(ticket).getByText('91.7% success')).toBeInTheDocument()
    expect(within(ticket).getByText('Healthy')).toBeInTheDocument()
    expect(within(ticket).getByText(/^Last run failed/)).toBeInTheDocument()
    expect(within(ticket).getByText(/seed-triage-router · seed-research-agent/)).toBeInTheDocument()
    expect(within(await card('Invoice triage')).getByText('Not run yet')).toBeInTheDocument()
    // Drafts live in their own list.
    expect(screen.queryByRole('link', { name: 'Onboarding checklist' })).toBeNull()
    expect(screen.getByRole('link', { name: copy.nav.deployed })).toHaveAttribute(
      'aria-current',
      'page',
    )
  })

  it('searches here and sorts on the server', async () => {
    const { urls, stop } = recordRequests()
    renderApp('/workflows')
    await card('Ticket resolution')
    await userEvent.type(
      screen.getByRole('searchbox', { name: copy.list.deployed.search }),
      'contr',
    )
    await waitFor(() =>
      expect(screen.queryByRole('link', { name: 'Ticket resolution' })).toBeNull(),
    )
    expect(screen.getByRole('link', { name: 'Contract intake' })).toBeInTheDocument()
    await userEvent.click(screen.getByRole('combobox', { name: copy.sortLabel }))
    await userEvent.click(await screen.findByRole('option', { name: 'Health' }))
    await waitFor(() =>
      expect(
        urls.some(
          (u) => u.pathname === '/api/maf/workflows' && u.searchParams.get('sort') === 'health',
        ),
      ).toBe(true),
    )
    stop()
  })

  it('lists drafts, and opens the other list from the sub-nav', async () => {
    renderApp('/workflows')
    await card('Ticket resolution')
    await userEvent.click(screen.getByRole('link', { name: copy.nav.drafts }))
    // Wait for the drafts list first: the page re-renders its sub-nav as the list loads, so a tab found before then
    // can be detached by the time it is checked (the flake this replaced).
    const onboarding = await card('Onboarding checklist')
    expect(screen.getByRole('link', { name: copy.nav.drafts, current: 'page' })).toBeVisible()
    expect(within(onboarding).getByText(/^Updated /)).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: 'Ticket resolution' })).toBeNull()
  })

  it('deletes behind a confirm, and the card goes', async () => {
    renderApp('/workflows')
    const invoice = await card('Invoice triage')
    await userEvent.click(within(invoice).getByRole('button', { name: /Workflow actions/ }))
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.delete }))
    const dialog = await screen.findByRole('alertdialog')
    expect(within(dialog).getByText(copy.deleteText('Invoice triage'))).toBeInTheDocument()
    await userEvent.click(within(dialog).getByRole('button', { name: copy.confirmDelete }))
    expect(await screen.findByText(copy.deleted, {}, T)).toBeInTheDocument()
    await waitFor(() => expect(screen.queryByRole('link', { name: 'Invoice triage' })).toBeNull())
  })

  it('Run now lands on the Runs tab with the new run open, and one Back returns', async () => {
    const { router } = renderApp('/workflows')
    const invoice = await card('Invoice triage')
    await userEvent.click(within(invoice).getByRole('button', { name: /Workflow actions/ }))
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.runNow }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/workflows/runs'))
    await waitFor(() => expect(router.state.location.search).toHaveProperty('run'), T)
    const run = (router.state.location.search as { run: string }).run
    await waitFor(() => {
      const el = document.querySelector(`[data-card="${run}"]`)
      expect(el).toHaveAttribute('data-open')
    }, T)
    router.history.back()
    await waitFor(() => expect(router.state.location.pathname).toBe('/workflows'))
  })

  it('shows the empty card with both ways forward', async () => {
    configureMocks({ variant: 'workflows-empty' })
    renderApp('/workflows')
    expect(await screen.findByText(copy.list.deployed.empty, {}, T)).toBeInTheDocument()
    expect(screen.getByRole('link', { name: copy.list.deployed.other })).toHaveAttribute(
      'href',
      '/workflows/drafts',
    )
  })

  it('on `main`: no metrics, no sort, and Drafts needs a newer server (W-1, W-3)', async () => {
    configureMocks({ variant: 'workflows-classic' })
    renderApp('/workflows')
    const ticket = await card('Ticket resolution')
    expect(within(ticket).getByText('12 runs')).toBeInTheDocument()
    expect(within(ticket).queryByText('Healthy')).toBeNull()
    expect(within(ticket).queryByText(/^Last run/)).toBeNull()
    expect(screen.queryByRole('combobox', { name: copy.sortLabel })).toBeNull()
    await userEvent.click(screen.getByRole('link', { name: copy.nav.drafts }))
    expect(await screen.findByText(copy.draftsAbsent, {}, T)).toBeInTheDocument()
  })
})
