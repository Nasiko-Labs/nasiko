/**
 * The Agents catalog's ?health= filter from the Overview (overview design 15A, eng R1; ship coverage audit):
 * the chip removes the filter, and an unknown rating in the URL is ignored.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import { setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'

setupPinnedSeed()

describe('Agents catalog ?health=', () => {
  it('the chip removes the filter and brings every agent back; a bogus rating is ignored', async () => {
    const { router } = renderApp('/agents?health=watch')
    const chip = await screen.findByRole('button', { name: 'Remove the Health: Watch filter' })
    const main = screen.getByRole('main')
    await waitFor(() => expect(within(main).getAllByRole('listitem').length).toBeGreaterThan(0))
    const filtered = within(main).getAllByRole('listitem').length
    await userEvent.click(chip)
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('health'))
    await waitFor(() =>
      expect(within(main).getAllByRole('listitem').length).toBeGreaterThan(filtered),
    )
    expect(within(main).queryAllByTestId('health-reason')).toHaveLength(0)

    await router.navigate({ to: '/agents', search: { health: 'bogus' } as never })
    await waitFor(() =>
      expect(within(screen.getByRole('main')).getAllByRole('listitem').length).toBeGreaterThan(
        filtered,
      ),
    )
    expect(screen.queryByRole('button', { name: /Remove the Health/ })).toBeNull()
  })
})
