/** The free assessment (`/assessment`) and the sidebar's ways to it and to help (AssessmentCta, HelpRows). */
import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import { copy as shell } from '@nasiko/app-core/shell/copy'
import { DOCS_URL, EXPERT_CALL_URL } from '@nasiko/app-core/links'
import { assessmentsMockState, configureMocks } from '@nasiko/mocks/handlers'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ variant: null }))

const field = (name: string) => screen.getByRole('textbox', { name: new RegExp(`^${name}`) })
const submit = () => userEvent.click(screen.getByRole('button', { name: copy.form.submit }))

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

  it('asks a team for its company, then offers the booking page with the answers to copy', async () => {
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
    const book = screen.getByRole('link', { name: new RegExp(copy.done.book) })
    expect(book).toHaveAttribute('href', EXPERT_CALL_URL)
    expect(book).toHaveAttribute('target', '_blank')
    const summary = screen.getByText(/Applying as: Company or team/)
    expect(summary).toHaveTextContent('Company: Analytical Engines')
    expect(summary).toHaveTextContent('Monthly token spend: Under $100')
    expect(summary).toHaveTextContent('Challenges: Lack of token usage visibility')

    // Edit answers comes back to what was typed.
    await userEvent.click(screen.getByRole('button', { name: copy.done.edit }))
    expect(field(copy.form.name)).toHaveValue('Ada Lovelace')
  })

  it('still reaches the booking step when the server cannot save the answers', async () => {
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
  })

  it('puts the help rows and the assessment card in the sidebar, the waitlist gone', async () => {
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 1440 })
    renderApp('/')
    await screen.findByRole('navigation', { name: 'Main' })
    const sidebar = document.querySelector<HTMLElement>('[data-slot="sidebar"]')!
    const s = within(sidebar)
    expect(s.getByRole('link', { name: new RegExp(`^${shell.help.docs}`) })).toHaveAttribute(
      'href',
      DOCS_URL,
    )
    expect(s.getByRole('link', { name: new RegExp(`^${shell.help.expert}`) })).toHaveAttribute(
      'href',
      EXPERT_CALL_URL,
    )
    expect(s.getAllByRole('link', { name: shell.help.cli })[0]).toHaveAttribute('href', '/cli')
    expect(s.getAllByRole('link', { name: shell.assessment.cta })[0]).toHaveAttribute(
      'href',
      '/assessment',
    )
    expect(s.queryByText(shell.waitlist.badge)).toBeNull()
  })
})
