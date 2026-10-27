/** Previous/Next inside the routing sheet (docs/superpowers/plans/2026-10-06-prev-next.md). */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@nasiko/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()
beforeEach(() => localStorage.setItem('openruntime.router.defaultsOpen', 'open'))
afterEach(() => configureMocks({ seed, now, loggedIn: true, superuser: null, routerVariants: [] }))

const ready = async () => {
  await screen.findByRole('table', { name: copy.agentsTitle })
  await waitFor(() => expect(screen.queryAllByText(copy.readingRouting)).toHaveLength(0))
}
const changeButtons = () =>
  screen
    .getAllByRole('button', { name: new RegExp(`^${copy.changeRouting}: `) })
    .filter((b) => !b.hasAttribute('disabled'))
const nameOf = (b: HTMLElement) =>
  b.getAttribute('aria-label')!.slice(`${copy.changeRouting}: `.length)
const sheet = () => screen.getByRole('dialog')
const sheetTitle = (name: string) =>
  within(sheet()).findByRole('heading', { name: copy.routingTitle(name) })
const position = (n: number, of: number) =>
  within(sheet()).getByRole('group', { name: `agent ${n} of ${of}` })
/** The sheet's form, once the agent's routing has loaded. */
const formReady = () => within(sheet()).findByRole('radiogroup', { name: copy.routeWith })

describe('routing sheet Previous/Next', () => {
  it('steps through the agents table inside the sheet, with the buttons and J/K', async () => {
    renderApp('/router')
    await ready()
    const names = changeButtons().map(nameOf)
    expect(names.length).toBeGreaterThan(2)
    await userEvent.click(changeButtons()[0]!)
    await sheetTitle(names[0]!)
    expect(position(1, names.length)).toBeInTheDocument()
    // Opening starts on the sheet, not on an arrow (whose tooltip would take the first Escape).
    expect(sheet()).toHaveFocus()
    await userEvent.click(within(sheet()).getByRole('button', { name: 'Next agent' }))
    await sheetTitle(names[1]!)
    await formReady()
    expect(position(2, names.length)).toBeInTheDocument()
    // The stepper never remounts, so focus stays on the arrow used.
    expect(within(sheet()).getByRole('button', { name: 'Next agent' })).toHaveFocus()
    await userEvent.keyboard('j')
    await sheetTitle(names[2]!)
    await userEvent.keyboard('k')
    await sheetTitle(names[1]!)
    expect(position(2, names.length)).toBeInTheDocument()
  })

  it('blocks stepping while the form has unsaved changes', async () => {
    renderApp('/router')
    await ready()
    const names = changeButtons().map(nameOf)
    await userEvent.click(changeButtons()[0]!)
    await formReady()
    const unchecked = within(sheet())
      .getAllByRole('radio')
      .find((r) => r.getAttribute('aria-checked') !== 'true' && !r.hasAttribute('disabled'))!
    await userEvent.click(unchecked)
    const next = within(sheet()).getByRole('button', { name: 'Next agent' })
    await waitFor(() => expect(next).toHaveAttribute('aria-disabled', 'true'))
    expect(next).toHaveAccessibleDescription(copy.saveBeforeStepping)
    await userEvent.click(next)
    await userEvent.click(unchecked)
    await userEvent.keyboard('j')
    expect(position(1, names.length)).toBeInTheDocument()
    expect(
      within(sheet()).getByRole('heading', { name: copy.routingTitle(names[0]!) }),
    ).toBeInTheDocument()
  })

  it("returns focus to the last agent's row on close", async () => {
    renderApp('/router')
    await ready()
    const names = changeButtons().map(nameOf)
    await userEvent.click(changeButtons()[0]!)
    await userEvent.click(await within(sheet()).findByRole('button', { name: 'Next agent' }))
    await sheetTitle(names[1]!)
    await userEvent.keyboard('{Escape}')
    await waitFor(() =>
      expect(
        screen.getByRole('button', { name: `${copy.changeRouting}: ${names[1]}` }),
      ).toHaveFocus(),
    )
  })

  it('shows no arrows when opened from the agent page', async () => {
    const agent = seed.agents.find((a) => a.display_name === 'Research Agent')!
    // The routing card is on the Details tab since the agent hub took the Overview.
    renderApp(`/agents/${agent.id}?tab=details`)
    const card = (await screen.findByRole('heading', { name: copy.cardTitle })).closest('section')!
    await userEvent.click(await within(card).findByRole('button', { name: copy.changeRouting }))
    await screen.findByRole('dialog', { name: copy.routingTitle('Research Agent') })
    expect(within(sheet()).queryByRole('button', { name: 'Next agent' })).toBeNull()
  })
})
