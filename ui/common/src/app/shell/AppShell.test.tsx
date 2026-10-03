/**
 * The app shell through the real router and query client (plans/feat-app-shell.md §8):
 * sidebar items and active state, shared context, collapse memory, the phone sheet, the footer
 * rows (status, account, sign out) and the routes that moved under the shell, plus the Nasiko header link, the
 * Chat full-height layout and the Status page.
 */
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { chatRegistry } from '@/features/chat/registry'
import { readDraft, writeDraft } from '@/features/chat/drafts'
import { apiFetch } from '@/lib/api/client'
import { configureMocks } from '@/mocks/handlers'
import { ADMIN_ID } from '@/mocks/seed-harness'
import { setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequests, server } from '@/test/setup'
import { SIGNED_OUT_KEY } from '@/lib/session'
import { copy } from './copy'
import { NAV_ITEMS } from './nav'
import { LOGOUT_TIMEOUT_MS, signOut } from './signOut'
import { readPrefs, resetThemeState, setAccent, setTheme } from './theme'

setupPinnedSeed()

const nav = () => screen.getByRole('navigation', { name: 'Main' })
const navLink = (name: string) => within(nav()).getByRole('link', { name })
const sidebarState = () =>
  document.querySelector('[data-slot="sidebar"]')?.getAttribute('data-state')
const sidebar = () => document.querySelector<HTMLElement>('[data-slot="sidebar"]')!
/** The open sidebar's brand link (the phone top bar has one too; the rail's mark is the Expand button). */
const sidebarBrand = () => within(sidebar()).getByRole('link', { name: copy.brand })
const wide = () => Object.defineProperty(window, 'innerWidth', { configurable: true, value: 1440 })
const clearCookie = () => {
  document.cookie = 'sidebar_state=; path=/; max-age=0'
}

const originalWidth = window.innerWidth
beforeEach(() => clearCookie())
afterEach(() => {
  clearCookie()
  configureMocks({ variant: null })
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: originalWidth })
  document.documentElement.removeAttribute('data-theme')
  // Theme state is module-level: never let one test's accent leak into the next.
  resetThemeState()
})

async function openAccountMenu(name = /^Account: admin$/) {
  await userEvent.click(await screen.findByRole('button', { name }))
}

describe('sidebar items', () => {
  it('lists every page with a name, even as the collapsed rail', async () => {
    renderApp('/tokenops')
    await screen.findByRole('navigation', { name: 'Main' })
    // jsdom is 1024 px wide: no cookie means the first-visit rail (design review 9A).
    expect(sidebarState()).toBe('collapsed')
    for (const n of NAV_ITEMS)
      expect(navLink(n.label)).toHaveAttribute('href', expect.stringContaining(n.to))
  })

  it.each([
    ['/tokenops', 'TokenOps'],
    ['/sessions', 'Sessions'],
    ['/harnesses', 'Harnesses'],
    ['/agents', 'Agents'],
    ['/agents/mine', 'Agents'],
    ['/deploy', 'Agents'],
    ['/builds', 'Agents'],
    ['/chat', 'Chat'],
    ['/', 'Overview'],
  ])('marks %s as %s with aria-current', async (url, label) => {
    renderApp(url)
    await waitFor(() => expect(navLink(label)).toHaveAttribute('aria-current', 'page'))
    // Exactly one current item on the whole page: the Nasiko link to Chat never competes (§3 rule 4).
    // (Pages may mark their own tabs; only the sidebar is checked.)
    expect(
      document.querySelector('[data-slot="sidebar"]')!.querySelectorAll('[aria-current="page"]'),
    ).toHaveLength(1)
    for (const n of NAV_ITEMS.filter((x) => x.label !== label))
      expect(navLink(n.label)).not.toHaveAttribute('aria-current')
    expect(navLink(label).closest('[data-active]')).toHaveAttribute('data-active', 'true')
  })

  it('carries the shared window to Harnesses only; Sessions and TokenOps start on their own', async () => {
    renderApp('/tokenops?preset=24h&compare=0')
    await screen.findByRole('navigation', { name: 'Main' })
    await waitFor(() =>
      expect(navLink('Harnesses')).toHaveAttribute('href', expect.stringContaining('preset=24h')),
    )
    // The other shared keys still cross into Sessions and TokenOps; the window doesn't.
    expect(navLink('Sessions').getAttribute('href')).toContain('compare=false')
    expect(navLink('Sessions').getAttribute('href')).not.toContain('preset')
    expect(navLink('TokenOps').getAttribute('href')).not.toContain('preset')
    expect(navLink('Chat').getAttribute('href')).toBe('/chat')
    expect(navLink('Agents').getAttribute('href')).toBe('/agents')
  })

  it('keeps the shared keys and drops page-only ones when following the links', async () => {
    const { router } = renderApp('/sessions?preset=7d&day=2026-03-18')
    await screen.findByRole('navigation', { name: 'Main' })
    await userEvent.click(navLink('TokenOps'))
    await waitFor(() => expect(router.state.location.pathname).toBe('/tokenops'))
    // TokenOps opens on its own 30 days, not Sessions' window.
    expect(router.state.location.search).toMatchObject({ preset: '30d' })
    expect(router.state.location.search).not.toHaveProperty('day')
    await userEvent.click(navLink('Harnesses'))
    await waitFor(() => expect(router.state.location.pathname).toBe('/harnesses'))
    expect(router.state.location.search).toMatchObject({ preset: '30d' })
  })

  it('shows a way back into the app on an unknown URL', async () => {
    renderApp('/no-such-page')
    await screen.findByRole('heading', { name: 'Page not found' })
    expect(screen.getByRole('link', { name: 'Back to Overview' })).toHaveAttribute('href', '/')
    expect(screen.getByRole('link', { name: 'Go to Chat' })).toHaveAttribute('href', '/chat')
  })

  it('has no Weave fixture gallery: Weave is EE only', async () => {
    renderApp('/weave')
    await screen.findByRole('heading', { name: 'Page not found' })
  })

  it('skip link moves focus to the page without touching the URL', async () => {
    const { router } = renderApp('/agents')
    await screen.findByRole('navigation', { name: 'Main' })
    const before = router.state.location.href
    await userEvent.click(screen.getByRole('button', { name: 'Skip to content' }))
    expect(document.activeElement).toBe(document.getElementById('main'))
    expect(router.state.location.href).toBe(before)
  })

  it('renders /login without the sidebar', async () => {
    renderApp('/login')
    await screen.findByRole('heading', { name: 'Sign in to Nasiko' })
    expect(screen.queryByRole('navigation', { name: 'Main' })).toBeNull()
  })

  it('shows Status inside the shell with no nav item active (eng D9)', async () => {
    renderApp('/status')
    await screen.findByRole('heading', { name: 'Backend status' })
    expect(
      within(nav())
        .queryAllByRole('link')
        .filter((l) => l.getAttribute('aria-current')),
    ).toHaveLength(0)
  })

  it.each(['/', '/status'])('sends a signed-out visitor from %s to /login', async (url) => {
    configureMocks({ loggedIn: false })
    const { router } = renderApp(url)
    await screen.findByRole('heading', { name: 'Sign in to Nasiko' })
    expect(router.state.location.pathname).toBe('/login')
    expect(router.state.location.search).toMatchObject({ redirect: url })
  })
})

describe('header brand link', () => {
  it('goes to the Overview through the router without marking itself current', async () => {
    wide()
    const { router } = renderApp('/agents')
    await screen.findByRole('navigation', { name: 'Main' })
    const brand = sidebarBrand()
    expect(brand).toHaveAttribute('href', '/')
    await userEvent.click(brand)
    await waitFor(() => expect(router.state.location.pathname).toBe('/'))
    expect(brand).not.toHaveAttribute('aria-current')
    expect(navLink('Overview')).toHaveAttribute('aria-current', 'page')
  })

  it('leaves a modified click (new tab) to the browser', async () => {
    wide()
    const { router } = renderApp('/agents')
    await screen.findByRole('navigation', { name: 'Main' })
    const brand = sidebarBrand()
    // A modified click is left to the browser: the handler doesn't cancel it.
    expect(fireEvent.click(brand, { ctrlKey: true })).toBe(true)
    expect(fireEvent.click(brand, { metaKey: true })).toBe(true)
    expect(router.state.location.pathname).toBe('/agents')
  })

  it('is the Expand button in the rail: the mark turns into the expand icon on hover, and a click expands', async () => {
    renderApp('/agents')
    await screen.findByRole('navigation', { name: 'Main' })
    expect(sidebarState()).toBe('collapsed')
    const header = document.querySelector<HTMLElement>('[data-slot="sidebar-header"]')!
    // One control, no second row: no brand link and no separate button beside it.
    expect(within(header).queryByRole('link')).toBeNull()
    const expand = within(header).getByRole('button')
    expect(expand).toHaveAccessibleName('Expand')
    expect(expand).toHaveAttribute('aria-keyshortcuts')
    expect(expand).toHaveClass('group/expand')
    await userEvent.click(expand)
    expect(sidebarState()).toBe('expanded')
    expect(sidebarBrand()).toBeInTheDocument()
    expect(within(header).getByRole('button', { name: 'Collapse' })).toBeInTheDocument()
  })
})

describe('layout', () => {
  it('fills the viewport on Chat and grows with the page elsewhere (plan §7.1)', async () => {
    const { router } = renderApp('/chat')
    await screen.findByRole('navigation', { name: 'Main' })
    const wrapper = () => document.querySelector('[data-slot="sidebar-wrapper"]')!
    expect(wrapper()).toHaveClass('h-dvh')
    await router.navigate({ to: '/agents' })
    await waitFor(() => expect(wrapper()).toHaveClass('min-h-screen'))
    expect(wrapper()).not.toHaveClass('h-dvh')
    expect(document.getElementById('main')).toHaveClass('max-w-page')
  })

  it.each(['/login', '/no-such-page'])(
    'pages outside the shell keep one <main> landmark: %s',
    async (url) => {
      configureMocks({ loggedIn: url !== '/login' })
      renderApp(url)
      await vi.waitFor(() => expect(screen.getAllByRole('main')).toHaveLength(1))
    },
  )
})

describe('collapse', () => {
  it('opens on a wide first visit, then remembers the choice in one cookie, on Chat too (eng D2)', async () => {
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 1440 })
    renderApp('/tokenops')
    await screen.findByRole('navigation', { name: 'Main' })
    expect(sidebarState()).toBe('expanded')
    await userEvent.click(screen.getByRole('button', { name: 'Collapse' }))
    expect(sidebarState()).toBe('collapsed')
    expect(document.cookie).toContain('sidebar_state=false')
    expect(screen.getByRole('button', { name: 'Expand' })).toHaveAttribute('aria-keyshortcuts')
    cleanup()
    renderApp('/chat')
    await screen.findByRole('navigation', { name: 'Main' })
    expect(sidebarState()).toBe('collapsed')
  })

  it('a stored choice wins over the width default, on every page', async () => {
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: 1440 })
    document.cookie = 'sidebar_state=false; path=/'
    renderApp('/chat')
    await screen.findByRole('navigation', { name: 'Main' })
    expect(sidebarState()).toBe('collapsed')
  })

  it('toggles with Ctrl+B / ⌘B', async () => {
    renderApp('/agents')
    await screen.findByRole('navigation', { name: 'Main' })
    expect(sidebarState()).toBe('collapsed')
    await userEvent.keyboard('{Control>}b{/Control}')
    expect(sidebarState()).toBe('expanded')
  })

  it('leaves Ctrl+B alone while typing', async () => {
    renderApp('/agents')
    const search = await screen.findByRole('searchbox').catch(() => screen.findByRole('textbox'))
    expect(sidebarState()).toBe('collapsed')
    await userEvent.click(search)
    await userEvent.keyboard('{Control>}b{/Control}')
    expect(sidebarState()).toBe('collapsed')
  })

  it('ignores ⌘B / Ctrl+B that something else already handled', async () => {
    renderApp('/agents')
    await screen.findByRole('navigation', { name: 'Main' })
    expect(sidebarState()).toBe('collapsed')
    const claim = (e: KeyboardEvent) => e.preventDefault()
    window.addEventListener('keydown', claim, { capture: true })
    try {
      await userEvent.keyboard('{Control>}b{/Control}')
      expect(sidebarState()).toBe('collapsed')
    } finally {
      window.removeEventListener('keydown', claim, { capture: true })
    }
    await userEvent.keyboard('{Control>}b{/Control}')
    expect(sidebarState()).toBe('expanded')
  })
})

describe('phone sheet', () => {
  it('opens from the top bar and closes on navigation', async () => {
    vi.stubGlobal('matchMedia', (q: string) => ({
      matches: q === '(max-width: 767px)',
      media: q,
      addEventListener: () => {},
      removeEventListener: () => {},
    }))
    const { router } = renderApp('/tokenops')
    await userEvent.click(await screen.findByRole('button', { name: 'Open navigation' }))
    const sheet = await screen.findByRole('dialog')
    await userEvent.click(within(sheet).getByRole('link', { name: 'Agents' }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/agents'))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
  })

  it('closes when the current page is picked again', async () => {
    vi.stubGlobal('matchMedia', (q: string) => ({
      matches: q === '(max-width: 767px)',
      media: q,
      addEventListener: () => {},
      removeEventListener: () => {},
    }))
    renderApp('/agents')
    await userEvent.click(await screen.findByRole('button', { name: 'Open navigation' }))
    const sheet = await screen.findByRole('dialog')
    await userEvent.click(within(sheet).getByRole('link', { name: 'Agents' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
  })

  it('returns focus to the menu button when closed with Escape', async () => {
    vi.stubGlobal('matchMedia', (q: string) => ({
      matches: q === '(max-width: 767px)',
      media: q,
      addEventListener: () => {},
      removeEventListener: () => {},
    }))
    renderApp('/tokenops')
    const opener = await screen.findByRole('button', { name: 'Open navigation' })
    await userEvent.click(opener)
    const sheet = await screen.findByRole('dialog')
    expect(sheet).toBeInTheDocument()
    await userEvent.keyboard('{Escape}')
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(document.activeElement).toBe(opener)
  })

  it('has no Collapse row on phones: the sheet opens from the top bar instead', async () => {
    vi.stubGlobal('matchMedia', (q: string) => ({
      matches: q === '(max-width: 767px)',
      media: q,
      addEventListener: () => {},
      removeEventListener: () => {},
    }))
    renderApp('/agents')
    await userEvent.click(await screen.findByRole('button', { name: 'Open navigation' }))
    const sheet = await screen.findByRole('dialog')
    expect(within(sheet).getByRole('navigation', { name: 'Main' })).toBeInTheDocument()
    expect(within(sheet).queryByRole('button', { name: /^(Collapse|Expand)$/ })).toBeNull()
  })
})

describe('status row', () => {
  it('says Connected in mock mode, linking to Status', async () => {
    renderApp('/agents')
    const row = await screen.findByTestId('status-row')
    await waitFor(() => expect(row).toHaveAttribute('data-state', 'connected'))
    expect(row).toHaveAttribute('href', '/status')
    expect(row).toHaveTextContent('MOCK DATA')
  })

  it('is the current page on /status (overview eng R6)', async () => {
    renderApp('/status')
    const row = await screen.findByTestId('status-row')
    await waitFor(() => expect(row).toHaveAttribute('aria-current', 'page'))
  })

  it('says Server unreachable under ?mock=server-down (eng D4)', async () => {
    configureMocks({ variant: 'server-down' })
    renderApp('/agents')
    const row = await screen.findByTestId('status-row')
    await waitFor(() => expect(row).toHaveAttribute('data-state', 'unreachable'))
    expect(row).toHaveTextContent('Server unreachable')
  })

  // Regression: ISSUE-004 (/qa 2026-09-28, .gstack/qa-reports/qa-report-localhost-2026-09-28.md): the expanded
  // row truncated "Server unreachable" to "Server unre…".
  it('shows a label short enough to fit beside the badge and keeps the full text for screen readers', async () => {
    configureMocks({ variant: 'server-down' })
    renderApp('/agents')
    const row = await screen.findByTestId('status-row')
    await waitFor(() => expect(row).toHaveAttribute('data-state', 'unreachable'))
    // Visible: one word, so "unreachable" is never the part that gets cut off at 240 px.
    const visible = [...row.querySelectorAll('[aria-hidden]')].map((e) => e.textContent).join(' ')
    expect(visible).toContain('Unreachable')
    expect(visible).not.toContain('Server')
    // Screen readers still hear the full state.
    expect(row.querySelector('.sr-only')).toHaveTextContent('Server unreachable')
    expect(screen.getByRole('link', { name: /Server unreachable/ })).toBe(row)
  })
})

describe('Status page', () => {
  it('says unreachable and how to start the server under ?mock=server-down', async () => {
    configureMocks({ variant: 'server-down' })
    renderApp('/status')
    await waitFor(() =>
      expect(screen.getByTestId('backend-state')).toHaveTextContent('unreachable'),
    )
    expect(screen.getByText('just run-stack')).toBeInTheDocument()
  })

  it('says connected in mock mode', async () => {
    renderApp('/status')
    await waitFor(() => expect(screen.getByTestId('backend-state')).toHaveTextContent('connected'))
  })
})

describe('status row: checking', () => {
  it('stays quiet for the first second of a slow check, then says Checking…', async () => {
    let release!: () => void
    const gate = new Promise<void>((r) => (release = r))
    server.use(
      http.get('/health', async () => {
        await gate
        return HttpResponse.text('ok')
      }),
    )
    renderApp('/agents')
    const row = await screen.findByTestId('status-row')
    expect(row).toHaveAttribute('data-state', 'quiet')
    await waitFor(() => expect(row).toHaveAttribute('data-state', 'checking'), { timeout: 2500 })
    expect(row).toHaveTextContent('Checking…')
    release()
    await waitFor(() => expect(row).toHaveAttribute('data-state', 'connected'))
  })
})

describe('footer', () => {
  it('has the account and no Settings or Theme row (Settings opens from the account menu)', async () => {
    renderApp('/agents')
    await screen.findByRole('navigation', { name: 'Main' })
    expect(within(sidebar()).queryByRole('link', { name: 'Settings' })).toBeNull()
    expect(within(sidebar()).queryByRole('button', { name: 'Theme' })).toBeNull()
  })

  it('the account menu has Theme, Settings and Sign out', async () => {
    const { router } = renderApp('/agents')
    await openAccountMenu()
    expect((await screen.findAllByRole('menuitem')).map((i) => i.textContent?.trim())).toEqual([
      'Theme',
      'Settings',
      'Sign out',
    ])
    await userEvent.click(screen.getByRole('menuitem', { name: 'Settings' }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/settings'))
  })

  it('the account menu’s Theme submenu sets mode and colour theme, and stays open between picks', async () => {
    renderApp('/agents')
    await openAccountMenu()
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Theme' }))
    const mode = await screen.findByRole('group', { name: 'Mode' })
    const theme = screen.getByRole('group', { name: 'Theme' })
    expect(within(theme).getByRole('menuitemradio', { name: 'Carbon' })).toBeChecked()
    await userEvent.click(within(theme).getByRole('menuitemradio', { name: 'Plum' }))
    expect(document.documentElement).toHaveAttribute('data-theme', 'plum')
    expect(readPrefs().accent).toBe('plum')
    await userEvent.click(within(mode).getByRole('menuitemradio', { name: 'Dark' }))
    expect(document.documentElement).toHaveClass('dark')
    // resetThemeState rehydrates from this test's storage, which now holds these picks.
    setAccent('carbon')
    setTheme('system')
  })

  it('Settings → Appearance: its Mode and Theme radio groups set the theme', async () => {
    renderApp('/settings/appearance')
    const mode = await screen.findByRole('radiogroup', { name: 'Mode' })
    const theme = screen.getByRole('radiogroup', { name: 'Theme' })
    expect(
      within(mode)
        .getAllByRole('radio')
        .map((r) => r.closest('label')?.textContent),
    ).toEqual(['System', 'Light', 'Dark'])
    expect(
      within(theme)
        .getAllByRole('radio')
        .map((r) => r.closest('label')?.textContent),
    ).toEqual(['Teal', 'Indigo', 'Plum', 'Carbon'])
    expect(within(theme).getByRole('radio', { name: 'Carbon' })).toBeChecked()
    await userEvent.click(within(theme).getByRole('radio', { name: 'Plum' }))
    expect(within(theme).getByRole('radio', { name: 'Plum' })).toBeChecked()
    expect(document.documentElement).toHaveAttribute('data-theme', 'plum')
    expect(readPrefs().accent).toBe('plum')
    await userEvent.click(within(mode).getByRole('radio', { name: 'Dark' }))
    expect(document.documentElement).toHaveClass('dark')
  })
})

describe('drill-in panel (one sidebar on Chat and Settings)', () => {
  it('shows Settings sections in the expanded sidebar instead of the app nav, and none beside the page', async () => {
    wide()
    renderApp('/settings')
    const sections = await screen.findByRole('navigation', { name: 'Settings sections' })
    expect(sidebar()).toContainElement(sections)
    expect(screen.getAllByRole('navigation', { name: 'Settings sections' })).toHaveLength(1)
    expect(screen.queryByRole('navigation', { name: 'Main' })).toBeNull()
    expect(within(sections).getByRole('link', { name: 'General' })).toHaveAttribute(
      'aria-current',
      'page',
    )
  })

  it('keeps the app nav in the collapsed rail; the page shows its sections itself', async () => {
    renderApp('/settings')
    const sections = await screen.findByRole('navigation', { name: 'Settings sections' })
    expect(sidebarState()).toBe('collapsed')
    expect(sidebar()).not.toContainElement(sections)
    expect(nav()).toBeInTheDocument()
  })

  it('Main menu shows the app nav over the panel, focused on the current item; that item brings the panel back', async () => {
    wide()
    renderApp('/chat')
    await userEvent.click(await screen.findByRole('button', { name: 'Back to main menu' }))
    // Gone from the sidebar, and not back beside the page either: the sidebar still holds the panel (hidden).
    expect(screen.queryByRole('link', { name: 'New chat' })).toBeNull()
    await waitFor(() => expect(document.activeElement).toBe(navLink('Chat')))
    expect(navLink('Chat')).toHaveAttribute('aria-current', 'page')
    await userEvent.click(navLink('Chat'))
    expect(await within(sidebar()).findByRole('link', { name: 'New chat' })).toBeInTheDocument()
    await waitFor(() =>
      expect(document.activeElement).toBe(
        within(sidebar()).getByRole('button', { name: 'Back to main menu' }),
      ),
    )
  })

  it('shows no Settings sections beside the page under Main menu; the account menu’s Settings brings them back', async () => {
    wide()
    renderApp('/settings')
    await userEvent.click(await screen.findByRole('button', { name: 'Back to main menu' }))
    expect(await screen.findByRole('navigation', { name: 'Main' })).toBeInTheDocument()
    expect(screen.queryByRole('navigation', { name: 'Settings sections' })).toBeNull()
    expect(screen.getByRole('heading', { level: 1, name: 'General' })).toBeInTheDocument()
    // The account menu's Settings brings the sections back, as the current nav item does for Chat.
    await openAccountMenu()
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Settings' }))
    const sections = await screen.findByRole('navigation', { name: 'Settings sections' })
    expect(sidebar()).toContainElement(sections)
    await waitFor(() =>
      expect(document.activeElement).toBe(
        within(sidebar()).getByRole('button', { name: 'Back to main menu' }),
      ),
    )
  })

  it('Back returns to the page the user came from, with its search, past moves inside the module', async () => {
    wide()
    const { router } = renderApp('/tokenops?preset=7d')
    await screen.findByRole('navigation', { name: 'Main' })
    await router.navigate({ to: '/chat' })
    await router.navigate({ to: '/settings' })
    await router.navigate({ to: '/settings/secrets' })
    // Settings → Secrets is a move inside Settings: Back skips it, and Chat is the page before Settings.
    const back = await screen.findByRole('button', { name: 'Back to Chat' })
    // It reads just "Back"; the name and tooltip say where to.
    expect(back).toHaveTextContent(/^Back$/)
    expect(back).toHaveAttribute('title', 'Back to Chat')
    await userEvent.click(back)
    await waitFor(() => expect(router.state.location.pathname).toBe('/chat'))
    await userEvent.click(await screen.findByRole('button', { name: 'Back to TokenOps' }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/tokenops'))
    expect(router.state.location.search).toMatchObject({ preset: '7d' })
    await waitFor(() => expect(document.activeElement).toBe(navLink('TokenOps')))
  })

  it('leaves the panel for the app nav on a page without one', async () => {
    wide()
    const { router } = renderApp('/settings')
    await screen.findByRole('navigation', { name: 'Settings sections' })
    await router.navigate({ to: '/agents' })
    expect(await screen.findByRole('navigation', { name: 'Main' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Back to main menu' })).toBeNull()
  })

  it('carries the chat history into the phone sheet', async () => {
    vi.stubGlobal('matchMedia', (q: string) => ({
      matches: q === '(max-width: 767px)',
      media: q,
      addEventListener: () => {},
      removeEventListener: () => {},
    }))
    renderApp('/chat')
    await userEvent.click(await screen.findByRole('button', { name: 'Open navigation' }))
    const sheet = await screen.findByRole('dialog')
    expect(await within(sheet).findByRole('link', { name: 'New chat' })).toBeInTheDocument()
    expect(within(sheet).getByRole('button', { name: 'Back to main menu' })).toBeInTheDocument()
  })
})

describe('account row', () => {
  it('says Account unavailable when /api/me fails, with Retry and a working Sign out (eng D7)', async () => {
    // Pages read `me` too, so the failure lasts until the server "comes back".
    let down = true
    server.use(
      http.get('/api/me', () => (down ? new HttpResponse('boom', { status: 500 }) : undefined)),
    )
    const { router } = renderApp('/agents')
    await openAccountMenu(/^Account unavailable$/)
    down = false
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Retry' }))
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Account: admin' })).toBeInTheDocument(),
    )
    await openAccountMenu()
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Sign out' }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/login'))
  })

  it('Retry sends the user to /login?expired=true when the retried /api/me says the session is gone', async () => {
    let status = 500
    server.use(http.get('/api/me', () => new HttpResponse('nope', { status })))
    const { router } = renderApp('/agents')
    await userEvent.click(await screen.findByRole('button', { name: /^Account unavailable$/ }))
    status = 401
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Retry' }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/login'))
    // Back to the same page after signing in (the router's location, not window.location).
    expect(router.state.location.search).toMatchObject({ expired: true, redirect: '/agents' })
  })

  it('a reset under the mounted shell shows "Loading account…" while me reloads, never the error state', async () => {
    const { queryClient } = renderApp('/agents')
    await screen.findByRole('button', { name: 'Account: admin' })
    let release!: () => void
    const gate = new Promise<void>((r) => (release = r))
    server.use(
      http.get('/api/me', async () => {
        await gate
        return HttpResponse.json({
          sub: '5eed0000-0000-4000-8000-00000000a001',
          username: 'admin',
          is_superuser: true,
        })
      }),
    )
    void queryClient.resetQueries({ queryKey: ['me'] })
    await userEvent.click(await screen.findByRole('button', { name: 'Loading account…' }))
    expect(screen.queryByRole('button', { name: 'Account unavailable' })).toBeNull()
    // No user yet: Sign out waits (it would clear every user's drafts), and there's nothing to retry.
    expect(await screen.findByRole('menuitem', { name: 'Sign out' })).toHaveAttribute(
      'aria-disabled',
      'true',
    )
    expect(
      screen.getByText('Loading your account. Sign out is available once it loads.'),
    ).toBeTruthy()
    expect(screen.queryByRole('menuitem', { name: 'Retry' })).toBeNull()
    await userEvent.keyboard('{Escape}')
    release()
    expect(await screen.findByRole('button', { name: 'Account: admin' })).toBeTruthy()
  })
})

describe('sign out (eng D6)', () => {
  it('clears chat state and the cache, ends the session, lands on plain /login, keeps the theme', async () => {
    setAccent('plum')
    const reqs = recordRequests()
    try {
      const { router, queryClient } = renderApp('/agents')
      await openAccountMenu()
      const registry = chatRegistry(queryClient, ADMIN_ID)
      const clearAll = vi.spyOn(registry, 'clearAll')
      writeDraft(ADMIN_ID, 'new:routed', 'half a thought')
      await userEvent.click(await screen.findByRole('menuitem', { name: 'Sign out' }))
      await waitFor(() => expect(router.state.location.pathname).toBe('/login'))
      expect(router.state.location.search).not.toHaveProperty('expired')
      expect(router.state.location.search).not.toHaveProperty('signout')
      expect(clearAll).toHaveBeenCalled()
      expect(readDraft(ADMIN_ID, 'new:routed')).toBe('')
      expect(queryClient.getQueryData(['agents'])).toBeUndefined()
      expect(reqs.urls.filter((u) => u.pathname === '/api/auth/logout')).toHaveLength(1)
      // The sign-in screen shows Carbon (theme.ts LOGIN_ACCENT); the stored choice is kept for after sign-in.
      await waitFor(() => expect(document.documentElement).toHaveAttribute('data-theme', 'carbon'))
      expect(readPrefs().accent === 'plum' || readPrefs().accent === 'carbon').toBe(true)
    } finally {
      reqs.stop()
    }
  })

  it('waits for logout before showing /login; a 401 meanwhile never means "expired"; no late logout', async () => {
    let release!: () => void
    const gate = new Promise<void>((r) => (release = r))
    server.use(
      http.post(
        '/api/auth/logout',
        async () => {
          await gate
          configureMocks({ loggedIn: false })
          return new HttpResponse(null, { status: 204 })
        },
        { once: true },
      ),
    )
    const reqs = recordRequests()
    try {
      const { router, queryClient } = renderApp('/agents')
      await openAccountMenu()
      await userEvent.click(await screen.findByRole('menuitem', { name: 'Sign out' }))
      expect(await screen.findByRole('menuitem', { name: 'Signing out…' })).toHaveAttribute(
        'aria-disabled',
        'true',
      )
      expect(router.state.location.pathname).toBe('/agents')
      // A refetch that 401s while signing out must not redirect to /login?expired=true.
      configureMocks({ loggedIn: false })
      await queryClient
        .fetchQuery({ queryKey: ['probe'], queryFn: () => apiFetch('/api/agents') })
        .catch(() => {})
      expect(router.state.location.pathname).toBe('/agents')
      release()
      await waitFor(() => expect(router.state.location.pathname).toBe('/login'))
      expect(router.state.location.search).not.toHaveProperty('expired')
      // Signing straight back in is never followed by another logout.
      await userEvent.click(await screen.findByRole('button', { name: 'Sign in' }))
      await waitFor(() => expect(router.state.location.pathname).toBe('/'))
      expect(reqs.urls.filter((u) => u.pathname === '/api/auth/logout')).toHaveLength(1)
    } finally {
      reqs.stop()
    }
  })

  it('a failed logout still lands on /login, says so, and Try again repeats only the logout', async () => {
    server.use(
      http.post('/api/auth/logout', () => new HttpResponse('down', { status: 500 }), {
        once: true,
      }),
    )
    const reqs = recordRequests()
    try {
      const { router, queryClient } = renderApp('/agents')
      await openAccountMenu()
      const clearAll = vi.spyOn(chatRegistry(queryClient, ADMIN_ID), 'clearAll')
      writeDraft(ADMIN_ID, 'new:routed', 'half a thought')
      await userEvent.click(await screen.findByRole('menuitem', { name: 'Sign out' }))
      await waitFor(() => expect(router.state.location.search).toMatchObject({ signout: 'failed' }))
      expect(router.state.location.pathname).toBe('/login')
      // Local state is gone even though the server didn't confirm.
      expect(clearAll).toHaveBeenCalled()
      expect(readDraft(ADMIN_ID, 'new:routed')).toBe('')
      expect(queryClient.getQueryCache().getAll()).toHaveLength(0)
      // The local barrier is set: this browser stays on /login until someone signs in (review D1).
      expect(localStorage.getItem(SIGNED_OUT_KEY)).toBe('1')
      // The notice never claims this browser is signed out: the HttpOnly cookie may still be valid.
      expect(screen.getByText(/this browser may still be signed in/)).toBeInTheDocument()
      expect(screen.queryByText(/signed out on this device/)).toBeNull()
      const before = reqs.urls.length
      await userEvent.click(screen.getByRole('button', { name: 'Try again' }))
      await waitFor(() => expect(screen.queryByText(/may still be signed in/)).toBeNull())
      expect(reqs.urls.slice(before).map((u) => u.pathname)).toEqual(['/api/auth/logout'])
      expect(router.state.location.search).not.toHaveProperty('signout')
    } finally {
      reqs.stop()
    }
  })

  it(`gives up on a logout that takes longer than ${LOGOUT_TIMEOUT_MS / 1000} s`, async () => {
    const timeouts: number[] = []
    const controllers: AbortController[] = []
    // Every request has apiFetch's own deadline; only the logout's cap is ours to fire.
    const real = AbortSignal.timeout.bind(AbortSignal)
    vi.spyOn(AbortSignal, 'timeout').mockImplementation((ms: number) => {
      if (ms !== LOGOUT_TIMEOUT_MS) return real(ms)
      timeouts.push(ms)
      const c = new AbortController()
      controllers.push(c)
      return c.signal
    })
    server.use(http.post('/api/auth/logout', () => new Promise<never>(() => {}), { once: true }))
    const { router } = renderApp('/agents')
    await openAccountMenu()
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Sign out' }))
    await waitFor(() => expect(timeouts).toEqual([LOGOUT_TIMEOUT_MS]))
    expect(router.state.location.pathname).toBe('/agents')
    controllers[0]!.abort(new DOMException('timed out', 'TimeoutError'))
    await waitFor(() => expect(router.state.location.search).toMatchObject({ signout: 'failed' }))
    expect(router.state.location.pathname).toBe('/login')
  })

  it('an expired session (logout 401) counts as signed out, with no failure notice', async () => {
    server.use(
      http.post(
        '/api/auth/logout',
        () =>
          HttpResponse.json(
            { data: null, status_code: 401, message: 'not authenticated' },
            { status: 401 },
          ),
        { once: true },
      ),
    )
    const { router } = renderApp('/agents')
    await openAccountMenu()
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Sign out' }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/login'))
    expect(router.state.location.search).not.toHaveProperty('signout')
    expect(router.state.location.search).not.toHaveProperty('expired')
  })

  it('Try again that fails again says so, stays retryable, and blocks sign-in meanwhile', async () => {
    let release!: () => void
    const gate = new Promise<void>((r) => (release = r))
    server.use(
      http.post('/api/auth/logout', async () => {
        await gate
        return new HttpResponse('down', { status: 500 })
      }),
    )
    // The notice shows only while the barrier is set (a failed logout set it).
    localStorage.setItem(SIGNED_OUT_KEY, '1')
    renderApp('/login?signout=failed')
    await userEvent.click(await screen.findByRole('button', { name: 'Try again' }))
    expect(screen.getByRole('button', { name: 'Sign in' })).toBeDisabled()
    release()
    expect(await screen.findByText('Still no answer from the server.')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Try again' })).toBeEnabled()
    expect(screen.getByRole('button', { name: 'Sign in' })).toBeEnabled()
  })

  it('signs out from Account unavailable too, clearing every draft', async () => {
    server.use(http.get('/api/me', () => new HttpResponse('boom', { status: 500 })))
    writeDraft('someone-else', 'new:routed', 'left behind')
    const { router } = renderApp('/agents')
    await openAccountMenu(/^Account unavailable$/)
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Sign out' }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/login'))
    expect(readDraft('someone-else', 'new:routed')).toBe('')
  })

  it('runs one sign-out at a time: a second call shares the first', async () => {
    const reqs = recordRequests()
    try {
      const { router, queryClient } = renderApp('/agents')
      await screen.findByRole('navigation', { name: 'Main' })
      const navigate = (to: { to: '/login'; search: { signout?: 'failed' } }) => router.navigate(to)
      const a = signOut({ queryClient, userId: ADMIN_ID, navigate })
      const b = signOut({ queryClient, userId: ADMIN_ID, navigate })
      expect(b).toBe(a)
      await a
      expect(reqs.urls.filter((u) => u.pathname === '/api/auth/logout')).toHaveLength(1)
    } finally {
      reqs.stop()
    }
  })

  it('after a failed logout this browser stays on /login until someone signs in (review D1)', async () => {
    const store = new Map<string, string>()
    vi.stubGlobal('localStorage', {
      get length() {
        return store.size
      },
      clear: () => store.clear(),
      getItem: (k: string) => store.get(k) ?? null,
      key: (i: number) => [...store.keys()][i] ?? null,
      removeItem: (k: string) => void store.delete(k),
      setItem: (k: string, v: string) => void store.set(k, String(v)),
    } satisfies Storage)
    server.use(
      http.post('/api/auth/logout', () => new HttpResponse('down', { status: 500 }), {
        once: true,
      }),
    )
    const { router } = renderApp('/agents')
    await openAccountMenu()
    await userEvent.click(await screen.findByRole('menuitem', { name: 'Sign out' }))
    await waitFor(() => expect(router.state.location.search).toMatchObject({ signout: 'failed' }))
    // The cookie still works (the mock stayed logged in), but reopening a page doesn't restore the user.
    await router.navigate({ to: '/chat' })
    await waitFor(() => expect(router.state.location.pathname).toBe('/login'))
    expect(router.state.location.search).toMatchObject({ signout: 'failed' })
    expect(screen.queryByRole('navigation', { name: 'Main' })).toBeNull()
    // A real sign-in lifts the barrier.
    await userEvent.click(await screen.findByRole('button', { name: 'Sign in' }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/chat'))
    expect(store.has('openruntime.signedOut')).toBe(false)
  })
})
