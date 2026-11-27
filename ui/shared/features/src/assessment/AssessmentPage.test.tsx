/** The free assessment (`/assessment`) and the sidebar's ways to it and to help (AssessmentCta, HelpRows). */
import { act, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import { copy as shell } from '@nasiko/app-core/shell/copy'
import { DOCS_URL, SALES_CALL_URL, SUPPORT_MAIL_URL } from '@nasiko/app-core/links'
import {
  assessmentBookingsMockState,
  assessmentsMockState,
  configureMocks,
} from '@nasiko/mocks/handlers'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ variant: null }))

const field = (name: string) => screen.getByRole('textbox', { name: new RegExp(`^${name}`) })
const submit = () => userEvent.click(screen.getByRole('button', { name: copy.form.submit }))

const EVENT = 'https://api.calendly.com/scheduled_events/AB12-cd34'
const INVITEE = `${EVENT}/invitees/EF56-gh78`

/** The Calendly embed on the last step, as a URL to read its query from. */
const bookingFrame = async () =>
  new URL((await screen.findByTitle(copy.done.frameTitle)).getAttribute('src')!)

/** What Calendly's embed posts to the page when the person books. */
function calendlyBooks(origin = 'https://calendly.com') {
  act(() => {
    window.dispatchEvent(
      new MessageEvent('message', {
        origin,
        data: {
          event: 'calendly.event_scheduled',
          payload: { event: { uri: EVENT }, invitee: { uri: INVITEE } },
        },
      }),
    )
  })
}

describe('assessment', () => {
  it('asks for the required answers before the last step', async () => {
    renderApp('/assessment')
    await screen.findByRole('heading', { level: 1, name: copy.headline })
    await submit()
    for (const m of [copy.errors.name, copy.errors.email, copy.errors.agents, copy.errors.spend])
      expect(await screen.findByText(m)).toBeInTheDocument()
    // An individual is never asked for a company.
    expect(screen.queryByRole('textbox', { name: /^Company/ })).toBeNull()
  })

  it('asks a team for its company, then embeds the Calendly booking tagged with the saved assessment', async () => {
    renderApp('/assessment')
    await screen.findByRole('heading', { level: 1, name: copy.headline })
    await userEvent.click(screen.getByRole('radio', { name: 'Company or team' }))
    await userEvent.type(field(copy.form.name), 'Ada Lovelace')
    await userEvent.type(field(copy.form.email), 'ada@example')
    await userEvent.click(screen.getByRole('radio', { name: '2–5 agents' }))
    await userEvent.click(screen.getByRole('radio', { name: 'Under $100' }))
    await userEvent.click(screen.getByRole('checkbox', { name: 'Lack of token usage visibility' }))
    await submit()
    expect(await screen.findByText(copy.errors.emailFormat)).toBeInTheDocument()
    expect(screen.getByText(copy.errors.company)).toBeInTheDocument()

    await userEvent.type(field(copy.form.email), '.com')
    await userEvent.type(field(copy.form.company), 'Analytical Engines')
    await submit()

    const step = await screen.findByRole('heading', { name: copy.done.title('Ada') })
    expect(step).toHaveFocus()
    expect(screen.getByText(new RegExp(copy.done.saved))).toBeInTheDocument()
    expect(assessmentsMockState()).toEqual([
      {
        applying_as: 'team',
        name: 'Ada Lovelace',
        email: 'ada@example.com',
        company: 'Analytical Engines',
        agents: '2-5',
        spend: '<100',
        challenges: ['Lack of token usage visibility'],
        notes: '',
      },
    ])
    const frame = await bookingFrame()
    expect(`${frame.origin}${frame.pathname}`).toBe(SALES_CALL_URL)
    expect(frame.searchParams.get('name')).toBe('Ada Lovelace')
    expect(frame.searchParams.get('email')).toBe('ada@example.com')
    expect(frame.searchParams.get('a1')).toBe('Analytical Engines')
    expect(frame.searchParams.get('utm_medium')).toBe('tokenops-assessment')
    expect(frame.searchParams.get('utm_content')).toMatch(/^[0-9a-f-]{36}$/)
    // Saved, so there are no answers to paste.
    expect(screen.queryByText(/Applying as:/)).toBeNull()

    // Edit answers comes back to what was typed.
    await userEvent.click(screen.getByRole('button', { name: copy.done.edit }))
    expect(field(copy.form.name)).toHaveValue('Ada Lovelace')
  })

  it('reports a Calendly booking for the saved assessment, and ignores other origins', async () => {
    renderApp('/assessment')
    await screen.findByRole('heading', { level: 1, name: copy.headline })
    await userEvent.type(field(copy.form.name), 'Ada')
    await userEvent.type(field(copy.form.email), 'ada@example.com')
    await userEvent.click(screen.getByRole('radio', { name: '1 agent' }))
    await userEvent.click(screen.getByRole('radio', { name: 'Under $100' }))
    await submit()
    const id = (await bookingFrame()).searchParams.get('utm_content')
    // An individual has no company to prefill.
    expect((await bookingFrame()).searchParams.get('a1')).toBeNull()

    calendlyBooks('https://evil.example')
    expect(screen.getByTitle(copy.done.frameTitle)).toBeInTheDocument()

    calendlyBooks()
    expect(await screen.findByRole('heading', { name: copy.booked.title })).toHaveFocus()
    expect(screen.queryByTitle(copy.done.frameTitle)).not.toBeInTheDocument()
    await waitFor(() =>
      expect(assessmentBookingsMockState()).toEqual([
        { id, body: { event_uri: EVENT, invitee_uri: INVITEE } },
      ]),
    )
  })

  it('still reaches the booking step, with the answers to paste, when the server cannot save them', async () => {
    configureMocks({ variant: 'assessment-unconfigured' })
    renderApp('/assessment')
    await screen.findByRole('heading', { level: 1, name: copy.headline })
    await userEvent.type(field(copy.form.name), 'Ada')
    await userEvent.type(field(copy.form.email), 'ada@example.com')
    await userEvent.click(screen.getByRole('radio', { name: '1 agent' }))
    await userEvent.click(screen.getByRole('radio', { name: 'Under $100' }))
    await submit()

    await screen.findByRole('heading', { name: copy.done.title('Ada') })
    expect(screen.getByText(new RegExp(copy.done.notSaved))).toBeInTheDocument()
    expect(assessmentsMockState()).toEqual([])
    // Nothing to tag the booking with, so the answers are shown to paste into its note, and a booking isn't reported.
    expect((await bookingFrame()).searchParams.get('utm_content')).toBeNull()
    expect(screen.getByText(/Applying as: Individual developer/)).toHaveTextContent(
      'Monthly token spend: Under $100',
    )
    calendlyBooks()
    await screen.findByRole('heading', { name: copy.booked.title })
    expect(assessmentBookingsMockState()).toEqual([])
  })

  it('puts the help rows in the sidebar and the assessment banner above the top bar, the waitlist gone', async () => {
    document.cookie = 'sidebar_state=true; path=/'
    renderApp('/')
    await screen.findByRole('navigation', { name: 'Main' })
    const sidebar = document.querySelector<HTMLElement>('[data-slot="sidebar"]')!
    const s = within(sidebar)
    expect(s.getByRole('link', { name: new RegExp(`^${shell.help.docs}`) })).toHaveAttribute(
      'href',
      DOCS_URL,
    )
    expect(s.getByRole('link', { name: shell.help.issue })).toHaveAttribute(
      'href',
      SUPPORT_MAIL_URL,
    )
    expect(s.getAllByRole('link', { name: shell.help.cli })[0]).toHaveAttribute('href', '/cli')
    expect(s.queryByRole('link', { name: shell.assessment.cta })).toBeNull()
    const banner = screen.getByRole('region', { name: shell.assessment.title })
    expect(within(banner).getByRole('link', { name: shell.assessment.cta })).toHaveAttribute(
      'href',
      '/assessment',
    )
    expect(s.queryByText(shell.waitlist.badge)).toBeNull()
    document.cookie = 'sidebar_state=; path=/; max-age=0'
  })
})
