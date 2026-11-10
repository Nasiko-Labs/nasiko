/**
 * The first-run guide (docs/superpowers/specs/2026-10-01-login-onboarding-design.md §2-§3, the Agent and key step from
 * 2026-11-04-quick-onboarding-design.md): a first-time user gets it on `/` with Home hidden; three steps, no role
 * (user request 2026-11-09); the Agent and key step's Continue waits a moment and deploys the prebuilt picks; it saves a
 * default config; skipping lasts the session; Finish lands on the homepage.
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

/** The Agent and key step's primary button, once its hold has counted down to `name`, clicked. */
async function continueAs(user: UserEvent, name: string) {
  const button = screen.getByRole('button', { name: /continue/i })
  await waitFor(() => expect(button).toHaveAccessibleName(name))
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
    // Welcome doesn't hold.
    on(screen.getByRole('button', { name: /get started/i }))
    fireEvent.click(screen.getByRole('button', { name: /get started/i }))
    // The seed user has a key, so only the hold keeps it off; then it deploys the prebuilt pick.
    const held = await screen.findByRole('button', { name: /^Continue \(\d\)$/ })
    off(held)
    await waitFor(() => expect(held).toHaveAccessibleName('Deploy 1 agent and continue'))
    on(held)
    // Any other tab still deploys the meeting notes processor, so one agent is always up.
    await user.click(screen.getByRole('tab', { name: 'Registry' }))
    expect(held).toHaveAccessibleName('Deploy 1 agent and continue')
  })

  it('has no role step: Get started opens the Agent and key step', async () => {
    firstTime()
    const user = userEvent.setup()
    const rec = recordRequestBodies()
    renderApp('/')
    await guide()
    const rail = screen.getByRole('navigation', { name: 'Getting started' })
    expect(
      within(rail)
        .getAllByRole('button')
        .map((b) => b.textContent),
    ).toEqual([
      expect.stringMatching(/Welcome/),
      expect.stringMatching(/Agent and key/),
      expect.stringMatching(/Coding harnesses/),
    ])
    await advance(user, /get started/i)
    await screen.findByRole('heading', { name: '2. Your model key' })
    expect(screen.queryByRole('radio', { name: /developer/i })).toBeNull()
    expect(screen.getByText('Step 2 of 3')).toBeInTheDocument()
    await rec.flush()
    expect(rec.requests.some((r) => r.method === 'PATCH')).toBe(false)
  })

  it('connects a provider as the default config with its key', async () => {
    firstTime()
    const user = userEvent.setup()
    const rec = recordRequestBodies()
    renderApp('/')
    await guide()
    await advance(user, /get started/i)
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

  it('ends on the coding harnesses and finishes on the homepage', async () => {
    firstTime()
    const user = userEvent.setup()
    const { router } = renderApp('/')
    await guide()
    await advance(user, /get started/i)
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
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    expect(router.state.location.pathname).toBe('/')
  })

  it('keeps a started deploy in the guide instead of opening its page', async () => {
    firstTime()
    const user = userEvent.setup()
    const { router } = renderApp('/')
    await guide()
    await advance(user, /get started/i)
    await screen.findByRole('heading', { name: '1. Your first agent' })
    await user.click(await screen.findByRole('tab', { name: 'Registry' }))
    // How to copy a reference from the public registry, above the form.
    const how = screen.getByRole('region', { name: 'How to copy from the registry' })
    expect(within(how).getByRole('link', { name: /registry\.nasiko\.dev/ })).toHaveAttribute(
      'href',
      'https://registry.nasiko.dev',
    )
    expect(within(how).getByText('docker pull')).toBeInTheDocument()
    await user.type(
      await screen.findByRole('textbox', { name: deployCopy.registry.reference }),
      'registry.nasiko.dev/nasiko/onboard-bot:1.0.0',
    )
    await user.click(screen.getByRole('button', { name: deployCopy.registry.submit }))
    expect(await screen.findByText(/Your agent is on its way/)).toBeInTheDocument()
    expect(router.state.location.pathname).toBe('/')
  })

  it('deploys up to two prebuilt agents on Continue, the meeting notes processor always included', async () => {
    firstTime()
    const user = userEvent.setup()
    const rec = recordRequestBodies()
    renderApp('/')
    await guide()
    await advance(user, /get started/i)
    const picks = await screen.findByRole('group', { name: 'Prebuilt agents to deploy' })
    const box = (name: RegExp) => within(picks).getByRole('checkbox', { name })
    // The meeting notes processor leads, checked and locked.
    expect(box(/Meeting Notes Processor/)).toBeChecked()
    expect(box(/Meeting Notes Processor/)).toBeDisabled()
    expect(within(picks).getAllByRole('checkbox')).toHaveLength(6)
    expect(
      screen.getByText(/Pick up to 2\. The one with the best record is always included\./),
    ).toBeInTheDocument()
    await user.click(box(/Summarizer/))
    expect(within(picks).getByText(/2 of 2 picked/)).toBeInTheDocument()
    // Full: picking another swaps it in rather than being locked out.
    expect(box(/Translator/)).toBeEnabled()
    await user.click(box(/Translator/))
    expect(box(/Translator/)).toBeChecked()
    expect(box(/Summarizer/)).not.toBeChecked()
    await user.click(box(/Summarizer/))
    // No Deploy button on the tab: the guide's Continue deploys the picks and moves on.
    expect(within(picks).queryByRole('button')).toBeNull()
    await continueAs(user, 'Deploy 2 agents and continue')
    await screen.findByRole('heading', { name: 'Bring in your coding harnesses' })
    // The guide keeps it across steps, and Continue is plain once they're on their way.
    await user.click(screen.getByRole('button', { name: 'Back' }))
    expect(
      await screen.findByText(/^Meeting Notes Processor and Summarizer are on their way/),
    ).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /^Continue( \(\d\))?$/ })).toBeInTheDocument()
    // Each one imported from the registry, once.
    await rec.flush()
    expect(
      rec.requests
        .filter((r) => r.method === 'POST' && r.url.pathname === '/api/import/registry')
        .map((r) => r.body),
    ).toEqual([
      { reference: 'registry.nasiko.dev/nasiko/meeting-notes-processor:1.0.0' },
      { reference: 'registry.nasiko.dev/nasiko/summarizer:1.0.0' },
    ])
  })

  it('never deploys the meeting notes processor again once it exists', async () => {
    firstTime()
    const user = userEvent.setup()
    const rec = recordRequestBodies()
    renderApp('/')
    await guide()
    await advance(user, /get started/i)
    const picks = await screen.findByRole('group', { name: 'Prebuilt agents to deploy' })
    await continueAs(user, 'Deploy 1 agent and continue')
    await screen.findByRole('heading', { name: 'Bring in your coding harnesses' })
    await user.click(screen.getByRole('button', { name: /finish/i }))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    // Back through the Setup guide: it now exists, so it shows as deployed and Continue deploys nothing.
    await user.click(await screen.findByRole('button', { name: 'Setup guide' }))
    // Key and agent both done: it reopens on the last step.
    await screen.findByRole('heading', { name: 'Bring in your coding harnesses' })
    await user.click(screen.getByRole('button', { name: 'Back' }))
    const again = await screen.findByRole('group', { name: 'Prebuilt agents to deploy' })
    expect(picks).not.toBe(again)
    await waitFor(() =>
      expect(
        within(again).getByRole('checkbox', { name: /Meeting Notes Processor.*Already deployed/ }),
      ).toBeInTheDocument(),
    )
    await user.click(screen.getByRole('tab', { name: 'Registry' }))
    await continueAs(user, 'Continue')
    await rec.flush()
    expect(
      rec.requests.filter((r) => r.method === 'POST' && r.url.pathname === '/api/import/registry'),
    ).toHaveLength(1)
  })

  it('turns Continue on after the hold with no key yet', async () => {
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
    await screen.findByRole('heading', { name: '2. Your model key' })
    expect(screen.queryByRole('button', { name: 'Skip this step' })).toBeNull()
    // No key is no reason to stay (user request 2026-11-06): Continue waits out the hold, then turns on.
    off(screen.getByRole('button', { name: /continue/i }))
    await waitFor(() => on(screen.getByRole('button', { name: /continue/i })))
    await user.click(screen.getByRole('radio', { name: /anthropic/i }))
    await user.type(screen.getByLabelText('Anthropic API key'), 'sk-ant-test')
    // A model the catalog doesn't list can be typed in, as the LLM router accepts.
    await user.type(screen.getByLabelText('Model'), 'claude-next')
    await user.click(screen.getByRole('button', { name: 'Connect' }))
    await screen.findByText(/Connected · Anthropic/)
    on(screen.getByRole('button', { name: /continue/i }))
  })

  it("says where to get a key, linking the provider's keys page", async () => {
    firstTime()
    const user = userEvent.setup()
    renderApp('/')
    await guide()
    await advance(user, /get started/i)
    await user.click(await screen.findByRole('radio', { name: /anthropic/i }))
    await user.click(screen.getByRole('button', { name: 'Where do I get a key?' }))
    expect(
      await screen.findByRole('link', { name: "Open Anthropic's API keys page" }),
    ).toHaveAttribute('href', 'https://platform.claude.com/settings/keys')
    expect(screen.getByText(/billed to the account that owns the key/)).toBeInTheDocument()
  })

  it('opens on the prebuilt agents, then the deploy tabs', async () => {
    firstTime()
    const user = userEvent.setup()
    renderApp('/')
    await guide()
    await advance(user, /get started/i)
    // Prebuilt, then Registry, GitHub and Upload a zip; the first is open.
    expect(await screen.findByRole('tab', { name: 'Prebuilt' })).toHaveAttribute(
      'aria-selected',
      'true',
    )
    expect(screen.getAllByRole('tab').map((t) => t.textContent)).toEqual([
      'Prebuilt',
      'Registry',
      'GitHub',
      'Upload a zip',
    ])
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
