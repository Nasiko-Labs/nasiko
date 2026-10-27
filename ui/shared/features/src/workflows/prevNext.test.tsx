/** Previous/Next on the workflow page (docs/superpowers/plans/2026-10-06-prev-next.md). */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, workflowsMockState } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { server } from '@nasiko/testing/setup'
import { workflowKeys } from './api'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

const T = { timeout: 8000 }
const titles = () =>
  screen
    .getAllByRole('link')
    .filter((l) => /^\/workflows\/[0-9a-f-]{36}$/.test(l.getAttribute('href') ?? ''))
const position = (n: number, of: number) =>
  screen.getByRole('group', { name: `workflow ${n} of ${of}` })

describe('workflow Previous/Next', () => {
  it('steps through the deployed list, replacing history', async () => {
    const { router } = renderApp('/workflows')
    await screen.findByRole('link', { name: 'Ticket resolution' }, T)
    const hrefs = titles().map((l) => l.getAttribute('href'))
    expect(hrefs.length).toBeGreaterThan(2)
    await userEvent.click(titles()[0]!)
    await userEvent.click(await screen.findByRole('button', { name: 'Next workflow' }, T))
    await waitFor(() => expect(router.state.location.pathname).toBe(hrefs[1]))
    await waitFor(() => expect(position(2, hrefs.length)).toBeInTheDocument())
    // The new page's arrow has focus again after the remount.
    await waitFor(() => expect(screen.getByRole('button', { name: 'Next workflow' })).toHaveFocus())
    await userEvent.keyboard('k')
    await waitFor(() => expect(router.state.location.pathname).toBe(hrefs[0]))
    router.history.back()
    await waitFor(() => expect(router.state.location.pathname).toBe('/workflows'))
  })

  it('asks before stepping away from a draft with unsaved edits', async () => {
    const { router } = renderApp('/workflows/drafts')
    await screen.findByRole('link', { name: copy.nav.drafts, current: 'page' }, T)
    await waitFor(() => expect(titles().length).toBeGreaterThan(1), T)
    const hrefs = titles().map((l) => l.getAttribute('href'))
    await userEvent.click(titles()[0]!)
    const name = await screen.findByLabelText(copy.nameLabel, {}, T)
    const typed = `${(name as HTMLInputElement).value} edited`
    await userEvent.type(name, ' edited')
    await userEvent.click(screen.getByRole('button', { name: 'Next workflow' }))
    const dialog = await screen.findByRole('alertdialog')
    expect(router.state.location.pathname).toBe(hrefs[0])
    // J inside the prompt does nothing: the arrows aren't in that dialog.
    await userEvent.keyboard('j')
    expect(router.state.location.pathname).toBe(hrefs[0])
    await userEvent.click(within(dialog).getByRole('button', { name: 'Stay' }))
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
    expect(router.state.location.pathname).toBe(hrefs[0])
    expect(screen.getByLabelText(copy.nameLabel)).toHaveValue(typed)
    expect(position(1, hrefs.length)).toBeInTheDocument()
  })

  it('keeps the arrows after a run view goes back to the workflow', async () => {
    const { router } = renderApp('/workflows')
    await screen.findByRole('link', { name: 'Ticket resolution' }, T)
    const hrefs = titles().map((l) => l.getAttribute('href'))
    const ids = hrefs.map((h) => h!.split('/').pop()!)
    const run = workflowsMockState().runs.find((r) => ids.includes(r.maf_id))
    expect(run).toBeDefined()
    const id = run!.maf_id
    const at = ids.indexOf(id)
    await userEvent.click(titles()[at]!)
    await screen.findByRole('button', { name: 'Next workflow' }, T)
    // Not pushed by the page (as from a link elsewhere): its Back replaces.
    void router.navigate({
      to: '/workflows/$workflowId',
      params: { workflowId: id },
      search: { run: run!.id },
      state: (s) => ({ siblings: s.siblings }),
    })
    await userEvent.click(await screen.findByRole('button', { name: copy.backToWorkflow }, T))
    await waitFor(() => expect(router.state.location.search).toEqual({}))
    expect(
      await screen.findByRole('group', { name: `workflow ${at + 1} of ${hrefs.length}` }, T),
    ).toBeInTheDocument()
  })

  it.each([
    [404, copy.notFound],
    [500, copy.loadOneFailed],
  ])('keeps the arrows on a workflow that fails to load (%i)', async (status, title) => {
    const { router, queryClient } = renderApp('/workflows')
    await screen.findByRole('link', { name: 'Ticket resolution' }, T)
    const hrefs = titles().map((l) => l.getAttribute('href'))
    expect(hrefs.length).toBeGreaterThan(2)
    const gone = hrefs[1]!.split('/').pop()!
    server.use(http.get(`/api/maf/workflow/${gone}`, () => new HttpResponse('failed', { status })))
    queryClient.removeQueries({ queryKey: workflowKeys.detail(gone) })
    await userEvent.click(titles()[0]!)
    await userEvent.click(await screen.findByRole('button', { name: 'Next workflow' }, T))
    expect(await screen.findByRole('heading', { level: 1, name: title }, T)).toBeInTheDocument()
    expect(position(2, hrefs.length)).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Next workflow' }))
    await waitFor(() => expect(router.state.location.pathname).toBe(hrefs[2]))
  })

  it('Save & run on edited steps goes to the run without the leave prompt', async () => {
    const id = workflowsMockState().workflows.find((w) => w.name === 'Onboarding checklist')!.id
    const { router } = renderApp(`/workflows/${id}`)
    const name = await screen.findByLabelText(copy.nameLabel, {}, T)
    await userEvent.type(name, ' edited')
    await userEvent.click(screen.getByRole('button', { name: copy.saveAndRun }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/workflows/runs'), T)
    expect(screen.queryByRole('alertdialog')).toBeNull()
  })

  it('shows no arrows on a deep link', async () => {
    const id = workflowsMockState().workflows.find((w) => w.name === 'Ticket resolution')!.id
    renderApp(`/workflows/${id}`)
    await screen.findByRole('heading', { level: 1, name: 'Ticket resolution' }, T)
    expect(screen.queryByRole('button', { name: 'Next workflow' })).toBeNull()
  })
})
