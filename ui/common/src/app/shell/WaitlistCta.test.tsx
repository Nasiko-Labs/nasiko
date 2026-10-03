/** The OSS waitlist links (WaitlistCta.tsx): the sidebar card, its rail row and the login line. */
import { screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { env } from '@/lib/env'
import { setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { copy } from './copy'

setupPinnedSeed()

const sidebar = () => document.querySelector<HTMLElement>('[data-slot="sidebar"]')!
const cardLink = { name: `${copy.waitlist.cta} ${copy.waitlist.newTab}` }
const railLink = { name: `${copy.waitlist.title} ${copy.waitlist.newTab}` }
const wide = () => Object.defineProperty(window, 'innerWidth', { configurable: true, value: 1440 })
const clearCookie = () => {
  document.cookie = 'sidebar_state=; path=/; max-age=0'
}

const originalWidth = window.innerWidth
beforeEach(() => clearCookie())
afterEach(() => {
  clearCookie()
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: originalWidth })
})

describe('waitlist', () => {
  it('links to the Nasiko waitlist page, tagged ref=oss-app', () => {
    expect(env.waitlistUrl).toBe('https://nasiko-waitlist.vercel.app/?ref=oss-app')
  })

  it('shows the Early access card in the expanded sidebar, opening the waitlist in a new tab', async () => {
    wide()
    renderApp('/')
    await screen.findByRole('navigation', { name: 'Main' })
    const link = within(sidebar()).getByRole('link', cardLink)
    expect(link).toHaveAttribute('href', env.waitlistUrl)
    expect(link).toHaveAttribute('target', '_blank')
    expect(link).toHaveAttribute('rel', 'noopener noreferrer')
    expect(within(sidebar()).getByText(copy.waitlist.badge)).toBeInTheDocument()
    expect(within(sidebar()).getByText(copy.waitlist.title)).toBeInTheDocument()
    // Outside the nav landmark: it isn't a page of the app.
    expect(
      within(screen.getByRole('navigation', { name: 'Main' })).queryByRole('link', cardLink),
    ).toBeNull()
  })

  it('keeps a ticket row for the collapsed rail, named for screen readers', async () => {
    renderApp('/agents')
    await screen.findByRole('navigation', { name: 'Main' })
    expect(sidebar()).toHaveAttribute('data-state', 'collapsed')
    expect(within(sidebar()).getByRole('link', railLink)).toHaveAttribute('href', env.waitlistUrl)
  })

  it('gives way to a page panel in the sidebar (Settings sections)', async () => {
    wide()
    renderApp('/settings')
    await screen.findByRole('navigation', { name: 'Settings sections' })
    expect(within(sidebar()).queryByRole('link', cardLink)).toBeNull()
    expect(within(sidebar()).queryByRole('link', railLink)).toBeNull()
  })

  it('adds one quiet line under Sign in', async () => {
    renderApp('/login')
    await screen.findByRole('heading', { name: 'Sign in to Nasiko' })
    const link = screen.getByRole('link', cardLink)
    expect(link).toHaveAttribute('href', env.waitlistUrl)
    expect(link).toHaveAttribute('target', '_blank')
    expect(link.closest('p')).toHaveTextContent(`${copy.waitlist.loginLead} ${copy.waitlist.cta}`)
  })
})
