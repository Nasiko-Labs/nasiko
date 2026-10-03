/** Background follow end to end (plans/feat-deploy.md §5, design review 8): leave the build, it finishes, one toast. */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, deployMockState } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { copy } from './copy'
import { clearUploads } from './uploads'
import { pollOnce } from './follower'

setupPinnedSeed()
afterEach(() => {
  // The follower and upload registry are module state: a followed build must not poll into the next test.
  clearUploads()
  configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null })
})

describe('Background follow', () => {
  it('counts the build on the Agents item (Builds is its sub-page), then toasts once when it finishes elsewhere', async () => {
    const { router } = renderApp('/deploy?method=github')
    const rows = await screen.findAllByTestId('repo-row', {}, { timeout: 5000 })
    await userEvent.click(
      within(rows.find((r) => r.textContent?.includes('acme/fresh-agent'))!).getByRole('radio'),
    )
    await userEvent.click(screen.getByRole('button', { name: 'Deploy acme/fresh-agent' }))
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/builds\//), {
      timeout: 5000,
    })
    const id = router.state.location.pathname.split('/').pop()!

    await router.navigate({ to: '/agents' })
    expect(await screen.findByTestId('nav-count-agents')).toHaveTextContent('1')
    expect(screen.getByRole('link', { name: /^Agents, 1 build in progress/ })).toBeInTheDocument()

    deployMockState().advance(id, 'success', 'completed')
    await pollOnce()
    expect(await screen.findByText(copy.build.running('fresh-agent'))).toBeInTheDocument()
    expect(screen.getByRole('button', { name: copy.build.chat })).toBeInTheDocument()
    expect(screen.queryByTestId('nav-count-agents')).toBeNull()
    await pollOnce()
    expect(screen.getAllByText(copy.build.running('fresh-agent'))).toHaveLength(1)
  })
})
