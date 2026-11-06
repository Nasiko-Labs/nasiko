/**
 * The first-run guide (docs/superpowers/specs/2026-10-01-login-onboarding-design.md §2-§3, the Agent and key step from
 * 2026-11-04-quick-onboarding-design.md): a first-time user gets it on `/` with Home hidden; each step's Continue waits a
 * moment; the persona saves on Continue; one step opens a running agent in Chat and saves a default config; skipping
 * lasts the session; the Coding harnesses step finishes on the role's page.
 */
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react'
import userEvent, { type UserEvent } from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { copy as deployCopy } from '@nasiko/features/deploy/copy'
import { clearUploads } from '@nasiko/features/deploy/uploads'
import { clearStoredDrafts } from '@nasiko/app-core/draftKeys'
import { copy as shellCopy } from '@nasiko/app-core/shell/copy'
import { configureMocks } from '@nasiko/mocks/handlers'
import { ADMIN_ID } from '@nasiko/mocks/seed-harness'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequestBodies, server } from '@nasiko/testing/setup'
import { skipKey } from './logic'

// The real 5 s hold would make every flow slow: 2 s still proves the countdown and outlasts a click under a loaded run.
vi.mock('./logic', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./logic')>()),
  CONTINUE_HOLD_SECONDS: 2,
}))

setupPinnedSeed()
afterEach(() => clearUploads())

// `variant: null`: a variant set by one test outlives it.
const firstTime = () =>
  configureMocks({
    onboarding: { persona: null, completed: false },
    variant: null,
  })
// The open guide is modal: Base UI hides the rest of the page from the accessibility tree.
const nav = () => screen.getByRole('navigation', { name: shellCopy.navLabel, hidden: true })
const guide = () => screen.findByRole('dialog', { name: /welcome to nasiko/i })
/** The primary button stays focusable while off, so it says so with aria-disabled. */
const off = (b: HTMLElement) => expect(b).toHaveAttribute('aria-disabled', 'true')
const on = (b: HTMLElement) => expect(b).not.toHaveAttribute('aria-disabled', 'true')
/** The step's primary button (Get started, Continue or Finish), once its wait is over. */
async function advance(user: UserEvent, name: RegExp = /continue/i) {
  const button = screen.getByRole('button', { name })
  await waitFor(() => on(button))
  await user.click(button)
}

describe('first-run guide', () => {
  it('stands in for Home and hides its nav item', async () => {
    firstTime()
    renderApp('/')
    await guide()
    expect(within(nav()).queryByRole('link', { name: 'Home', hidden: true })).toBeNull()
    // Chat still opens the nav, so Fleet keeps its header.
    expect(within(nav()).getByRole('link', { name: 'Chat', hidden: true })).toBeInTheDocument()
    expect(within(nav()).getByText(shellCopy.nav.fleet)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Setup guide', hidden: true })).toBeNull()
    await waitFor(() => expect(screen.getByRole('button', { name: /get started/i })).toHaveFocus())
  })

  it('holds only the Agent and key step, counting down in the label', async () => {
    firstTime()
    const user = userEvent.setup()
    renderApp('/')
    await guide()
    // OpenRuntime's narrative and its four stages come before the capabilities.
    const stages = screen.getByRole('list', { name: 'How work flows through OpenRuntime' })
    expect(
      within(stages)
        .getAllByRole('listitem')
        .map((li) => li.textContent),
    ).toEqual([
      expect.stringMatching(/^Connect01/),
      expect.stringMatching(/^Build02/),
      expect.stringMatching(/^Run03/),
      expect.stringMatching(/^Govern04/),
    ])
    // Welcome and Role don't hold.
    on(screen.getByRole('button', { name: /get started/i }))
    await user.click(screen.getByRole('button', { name: /get started/i }))
    fireEvent.click(screen.getByRole('radio', { name: /developer/i }))
    on(screen.getByRole('button', { name: /continue/i }))
    await user.click(screen.getByRole('button', { name: /continue/i }))
    // The seed user has a key, so only the hold keeps it off.
    const held = await screen.findByRole('button', { name: /^Continue \(\d\)$/ })
    off(held)
    await waitFor(() => expect(held).toHaveAccessibleName('Continue'))
    on(held)
  })

  it('saves the role on Continue and moves to the Key and agent step', async () => {
    firstTime()
    const user = userEvent.setup()
    const rec = recordRequestBodies()
    renderApp('/')
    await guide()
    await advance(user, /get started/i)
    off(screen.getByRole('button', { name: /continue/i }))
    await user.click(screen.getByRole('radio', { name: /finops \/ finance/i }))
    await advance(user)
    await screen.findByRole('heading', { name: '2. Your model key' })
    await rec.flush()
    const patch = rec.requests.find(
      (r) => r.method === 'PATCH' && r.url.pathname === '/api/me/onboarding',
    )
    expect(patch?.body).toEqual({ persona: 'finance' })
  })

  it('connects a provider as the default config with its key', async () => {
    firstTime()
    const user = userEvent.setup()
    const rec = recordRequestBodies()
    renderApp('/')
    await guide()
    await advance(user, /get started/i)
    await user.click(screen.getByRole('radio', { name: /developer/i }))
    await advance(user)
    await user.click(await screen.findByRole('radio', { name: /anthropic/i }))
    await user.type(screen.getByLabelText('Anthropic API key'), 'sk-ant-test')
    // The eye shows the pasted key, as on the login page.
    expect(screen.getByLabelText('Anthropic API key')).toHaveAttribute('type', 'password')
    await user.click(screen.getByRole('button', { name: 'Show key' }))
    expect(screen.getByLabelText('Anthropic API key')).toHaveAttribute('type', 'text')
    // The model is asked for, never guessed from the catalog (user 2026-11-06).
    await user.click(screen.getByRole('button', { name: 'Connect' }))
    expect(screen.getByText('Pick a model first.')).toBeInTheDocument()
    // The router's field: the provider's catalog models as suggestions, in the theme's own list.
    await user.click(screen.getByLabelText('Model'))
    await user.click(await screen.findByRole('option', { name: 'claude-sonnet-4' }))
    expect(screen.getByLabelText('Model')).toHaveValue('claude-sonnet-4')
    await user.click(screen.getByRole('button', { name: 'Connect' }))
    expect(await screen.findByText(/Connected · Anthropic · claude-sonnet-4/)).toBeInTheDocument()
    await rec.flush()
    const posts = rec.requests.filter(
      (r) => r.method === 'POST' && r.url.pathname === '/api/llm-configs',
    )
    expect(posts).toHaveLength(1)
    expect(posts[0]?.body).toMatchObject({
      provider: 'anthropic',
      model: 'claude-sonnet-4',
      api_key_secret_name: 'ANTHROPIC_API_KEY',
      secret_value: 'sk-ant-test',
      is_default: true,
    })
  })

  it('opens on its own only once, finished or not: a new sign-in does not bring it back', async () => {
    firstTime()
    renderApp('/')
    await guide()
    expect(localStorage.getItem(skipKey(ADMIN_ID))).toBe('1')
    cleanup()
    // Sign-out clears drafts; the flag isn't one. Still first-time on the server, but the guide stays shut.
    clearStoredDrafts()
    renderApp('/')
    expect(await screen.findByRole('button', { name: 'Setup guide' })).toBeInTheDocument()
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('skips for the session: Home and its nav item come back', async () => {
    firstTime()
    const user = userEvent.setup()
    renderApp('/')
    await guide()
    await user.click(screen.getByRole('button', { name: 'Skip guide' }))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(await screen.findByRole('button', { name: 'Setup guide' })).toBeInTheDocument()
    expect(within(nav()).getByRole('link', { name: 'Home' })).toBeInTheDocument()
    expect(within(nav()).getByText(shellCopy.nav.fleet)).toBeInTheDocument()
    expect(localStorage.getItem(skipKey(ADMIN_ID))).toBe('1')
  })

  it("ends on the coding harnesses and finishes on the role's page", async () => {
    firstTime()
    const user = userEvent.setup()
    const { router } = renderApp('/')
    await guide()
    await advance(user, /get started/i)
    await user.click(screen.getByRole('radio', { name: /sre/i }))
    await advance(user)
    await screen.findByRole('heading', { name: '2. Your model key' })
    // The seed user's model config counts as their key, so Continue comes on once it loads.
    await advance(user)
    await screen.findByRole('heading', { name: 'Bring in your coding harnesses' })
    // Install, then sign in (no SSO in this edition); nothing else.
    const commands = screen.getAllByText(/^(curl|nasiko) /).map((c) => c.textContent)
    expect(commands).toEqual([
      'curl -fsSL https://registry.nasiko.dev/r/nasiko/install | bash',
      'nasiko auth login',
    ])
    expect(screen.queryByRole('button', { name: 'Skip guide' })).toBeNull()
    // More ways on than the harnesses.
    expect(
      within(screen.getByRole('heading', { name: 'Or jump straight in' }).parentElement!)
        .getAllByRole('button')
        .map((b) => b.textContent),
    ).toEqual([
      expect.stringMatching(/^Open Coding harnesses/),
      expect.stringMatching(/^Chat with an agent/),
      expect.stringMatching(/^Open Agents/),
    ])
    await advance(user, /finish/i)
    await waitFor(() => expect(router.state.location.pathname).toBe('/sessions'))
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('keeps a started deploy in the guide instead of opening its page', async () => {
    firstTime()
    const user = userEvent.setup()
    const { router } = renderApp('/')
    await guide()
    await advance(user, /get started/i)
    await user.click(screen.getByRole('radio', { name: /developer/i }))
    await advance(user)
    await screen.findByRole('heading', { name: '1. Your first agent' })
    await user.click(await screen.findByRole('tab', { name: 'Registry' }))
    await user.type(
      await screen.findByRole('textbox', { name: deployCopy.registry.reference }),
      'registry.nasiko.dev/nasiko/onboard-bot:1.0.0',
    )
    await user.click(screen.getByRole('button', { name: deployCopy.registry.submit }))
    expect(await screen.findByText(/Your agent is on its way/)).toBeInTheDocument()
    expect(router.state.location.pathname).toBe('/')
  })

  it('opens a running agent in Chat straight from the Agent and key step', async () => {
    firstTime()
    const user = userEvent.setup()
    const { router } = renderApp('/')
    await guide()
    await advance(user, /get started/i)
    await user.click(screen.getByRole('radio', { name: /product manager/i }))
    await advance(user)
    // The seed user already has a model config, so Chat is on.
    const running = await screen.findByRole('list', { name: 'Agents running now' })
    expect(screen.queryByText(/Add your key below to chat/)).toBeNull()
    const [first] = within(running).getAllByRole('button', { name: /^Chat with / })
    await user.click(first!)
    await waitFor(() => expect(router.state.location.pathname).toBe('/chat'))
    expect(router.state.location.search).toMatchObject({ agent: expect.any(String) })
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('keeps Chat off until the user has a key, while Continue turns on after the hold anyway', async () => {
    firstTime()
    server.use(
      http.get('*/api/llm-configs', () =>
        HttpResponse.json({ data: [], status_code: 200, message: 'ok' }),
      ),
    )
    const user = userEvent.setup()
    renderApp('/')
    await guide()
    await advance(user, /get started/i)
    await user.click(screen.getByRole('radio', { name: /developer/i }))
    await advance(user)
    const running = await screen.findByRole('list', { name: 'Agents running now' })
    expect(screen.getByText(/Add your key below to chat/)).toBeInTheDocument()
    for (const b of within(running).getAllByRole('button', { name: /^Chat with / })) {
      expect(b).toBeDisabled()
    }
    expect(screen.queryByRole('button', { name: 'Skip this step' })).toBeNull()
    // No key is no reason to stay (user request 2026-11-06): Continue waits out the hold, then turns on.
    off(screen.getByRole('button', { name: /continue/i }))
    await waitFor(() => on(screen.getByRole('button', { name: /continue/i })))
    // A key connected here turns Chat on.
    await user.click(screen.getByRole('radio', { name: /anthropic/i }))
    await user.type(screen.getByLabelText('Anthropic API key'), 'sk-ant-test')
    // A model the catalog doesn't list can be typed in, as the LLM router accepts.
    await user.type(screen.getByLabelText('Model'), 'claude-next')
    await user.click(screen.getByRole('button', { name: 'Connect' }))
    await screen.findByText(/Connected · Anthropic/)
    expect(screen.queryByText(/Add your key below to chat/)).toBeNull()
    const again = screen.getByRole('list', { name: 'Agents running now' })
    for (const b of within(again).getAllByRole('button', { name: /^Chat with / })) {
      expect(b).toBeEnabled()
    }
    on(screen.getByRole('button', { name: /continue/i }))
  })

  it("says where to get a key, linking the provider's keys page", async () => {
    firstTime()
    const user = userEvent.setup()
    renderApp('/')
    await guide()
    await advance(user, /get started/i)
    await user.click(screen.getByRole('radio', { name: /developer/i }))
    await advance(user)
    await user.click(await screen.findByRole('radio', { name: /anthropic/i }))
    await user.click(screen.getByRole('button', { name: 'Where do I get a key?' }))
    expect(
      await screen.findByRole('link', { name: "Open Anthropic's API keys page" }),
    ).toHaveAttribute('href', 'https://platform.claude.com/settings/keys')
    expect(screen.getByText(/billed to the account that owns the key/)).toBeInTheDocument()
  })

  it('opens on the deploy tabs when no agent is running', async () => {
    firstTime()
    configureMocks({ variant: 'no-agents' })
    const user = userEvent.setup()
    renderApp('/')
    await guide()
    await advance(user, /get started/i)
    await user.click(screen.getByRole('radio', { name: /developer/i }))
    await advance(user)
    // Registry, GitHub, then Upload a zip; the first is open.
    expect(await screen.findByRole('tab', { name: 'Registry' })).toHaveAttribute(
      'aria-selected',
      'true',
    )
    expect(screen.getAllByRole('tab').map((t) => t.textContent)).toEqual([
      'Registry',
      'GitHub',
      'Upload a zip',
    ])
    expect(screen.queryByRole('tab', { name: 'Running now' })).toBeNull()
  })

  it('never opens for a finished user or a server without the endpoint', async () => {
    renderApp('/')
    expect(await screen.findByRole('button', { name: 'Setup guide' })).toBeInTheDocument()
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('treats an older server (bare 404) as finished', async () => {
    configureMocks({ variant: 'onboarding-absent' })
    renderApp('/')
    // The home paints before the onboarding read settles, so the button turns into the older server's sheet trigger.
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Setup guide' })).toHaveAttribute(
        'aria-haspopup',
        'dialog',
      ),
    )
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(within(nav()).getByRole('link', { name: 'Home' })).toBeInTheDocument()
  })
})
