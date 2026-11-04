/**
 * The first-run guide (docs/superpowers/specs/2026-10-01-login-onboarding-design.md §2-§3, the Key and agent step from
 * 2026-11-04-quick-onboarding-design.md): a first-time user gets it on `/` with Home hidden; the persona saves on
 * Continue; one step saves a default config and opens a running agent in Chat; skipping lasts the session; Ready leads
 * with Chat and Agents.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { copy as deployCopy } from '@nasiko/features/deploy/copy'
import { clearUploads } from '@nasiko/features/deploy/uploads'
import { copy as shellCopy } from '@nasiko/app-core/shell/copy'
import { configureMocks } from '@nasiko/mocks/handlers'
import { ADMIN_ID } from '@nasiko/mocks/seed-harness'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import { recordRequestBodies, server } from '@nasiko/testing/setup'
import { skipKey } from './logic'

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

describe('first-run guide', () => {
  it('stands in for Home and hides its nav item', async () => {
    firstTime()
    renderApp('/')
    await guide()
    expect(within(nav()).queryByRole('link', { name: 'Home', hidden: true })).toBeNull()
    // Home was the first group's only item: Fleet opens the nav, without a header (PR3 review).
    expect(within(nav()).getByRole('link', { name: 'Agents', hidden: true })).toBeInTheDocument()
    expect(within(nav()).queryByText(shellCopy.nav.fleet)).toBeNull()
    expect(screen.queryByRole('button', { name: 'Setup guide', hidden: true })).toBeNull()
    await waitFor(() => expect(screen.getByRole('button', { name: /get started/i })).toHaveFocus())
  })

  it('saves the role on Continue and moves to the Key and agent step', async () => {
    firstTime()
    const user = userEvent.setup()
    const rec = recordRequestBodies()
    renderApp('/')
    await guide()
    await user.click(screen.getByRole('button', { name: /get started/i }))
    const cont = screen.getByRole('button', { name: /continue/i })
    expect(cont).toBeDisabled()
    await user.click(screen.getByRole('radio', { name: /finops \/ finance/i }))
    await user.click(cont)
    await screen.findByRole('heading', { name: '1. Your model key' })
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
    await user.click(screen.getByRole('button', { name: /get started/i }))
    await user.click(screen.getByRole('radio', { name: /developer/i }))
    await user.click(screen.getByRole('button', { name: /continue/i }))
    await user.click(await screen.findByRole('radio', { name: /anthropic/i }))
    await user.type(screen.getByLabelText('Anthropic API key'), 'sk-ant-test')
    // The eye shows the pasted key, as on the login page.
    expect(screen.getByLabelText('Anthropic API key')).toHaveAttribute('type', 'password')
    await user.click(screen.getByRole('button', { name: 'Show key' }))
    expect(screen.getByLabelText('Anthropic API key')).toHaveAttribute('type', 'text')
    await user.click(screen.getByRole('button', { name: 'Connect' }))
    expect(await screen.findByText(/Connected · Anthropic/)).toBeInTheDocument()
    await rec.flush()
    const post = rec.requests.find(
      (r) => r.method === 'POST' && r.url.pathname === '/api/llm-configs',
    )
    expect(post?.body).toMatchObject({
      provider: 'anthropic',
      api_key_secret_name: 'ANTHROPIC_API_KEY',
      secret_value: 'sk-ant-test',
      is_default: true,
    })
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

  it("ends on Ready and opens the role's page", async () => {
    firstTime()
    const user = userEvent.setup()
    const { router } = renderApp('/')
    await guide()
    await user.click(screen.getByRole('button', { name: /get started/i }))
    await user.click(screen.getByRole('radio', { name: /sre/i }))
    await user.click(screen.getByRole('button', { name: /continue/i }))
    await screen.findByRole('heading', { name: '1. Your model key' })
    // The seed user's model config counts as their key, so Continue comes on once it loads.
    await waitFor(() => expect(screen.getByRole('button', { name: /continue/i })).toBeEnabled())
    await user.click(screen.getByRole('button', { name: /continue/i }))
    // Spend less: informational, so its opt-out is worded as a decision, not as skipping a task.
    await screen.findByRole('heading', { name: 'Cut what your agents spend' })
    await user.click(screen.getByRole('button', { name: 'I will do this later' }))
    await screen.findByRole('heading', { name: 'Your workspace is ready' })
    expect(screen.getByText('SRE / On-call')).toBeInTheDocument()
    // A key from before the guide is the model, not a skipped step.
    expect(screen.getAllByText('Skipped')).toHaveLength(1)
    // Chat and Agents lead; the role's own page follows.
    expect(screen.getByRole('button', { name: /chat with an agent/i })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /open agents/i })).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: /open sessions/i }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/sessions'))
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('keeps a started deploy in the guide instead of opening its page', async () => {
    firstTime()
    const user = userEvent.setup()
    const { router } = renderApp('/')
    await guide()
    await user.click(screen.getByRole('button', { name: /get started/i }))
    await user.click(screen.getByRole('radio', { name: /developer/i }))
    await user.click(screen.getByRole('button', { name: /continue/i }))
    await screen.findByRole('heading', { name: '2. Your first agent' })
    await user.click(await screen.findByRole('tab', { name: 'Registry' }))
    await user.type(
      await screen.findByRole('textbox', { name: deployCopy.registry.reference }),
      'registry.nasiko.dev/nasiko/onboard-bot:1.0.0',
    )
    await user.click(screen.getByRole('button', { name: deployCopy.registry.submit }))
    expect(await screen.findByText(/Your agent is on its way/)).toBeInTheDocument()
    expect(router.state.location.pathname).toBe('/')
    await user.click(screen.getByRole('button', { name: /continue/i }))
    await screen.findByRole('heading', { name: 'Cut what your agents spend' })
    await user.click(screen.getByRole('button', { name: /continue/i }))
    expect(await screen.findByText('Building')).toBeInTheDocument()
  })

  it('teaches the optimization switches and says where each one lives', async () => {
    // The only place in the product that tells a new user these exist: they are one tab deep on an
    // agent and every one is off by default, so a feature nobody discovers may as well not ship.
    firstTime()
    const user = userEvent.setup()
    const { router } = renderApp('/')
    await guide()
    await user.click(screen.getByRole('button', { name: /get started/i }))
    await user.click(screen.getByRole('radio', { name: /sre/i }))
    await user.click(screen.getByRole('button', { name: /continue/i }))
    await screen.findByRole('heading', { name: '1. Your model key' })
    // The seed user's model config counts as their key, so Continue comes on once it loads.
    await waitFor(() => expect(screen.getByRole('button', { name: /continue/i })).toBeEnabled())
    await user.click(screen.getByRole('button', { name: /continue/i }))

    const step = await screen.findByRole('heading', { name: 'Cut what your agents spend' })
    expect(step).toBeInTheDocument()
    // Each switch is named for what it trims, with the path to find it.
    for (const name of [
      'Smaller prompts',
      'Shorter chat history',
      'Less code written',
      'A smaller model when it will do',
    ]) {
      expect(screen.getByText(name)).toBeInTheDocument()
    }
    expect(screen.getByText(/Agent → Settings → Token optimization/)).toBeInTheDocument()
    expect(screen.getByText(/Optimization → Your settings/)).toBeInTheDocument()
    expect(screen.getByText(/LLM router → Configs → Tier models/)).toBeInTheDocument()
    expect(screen.getByText(/Try one on a single agent first/)).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Open agent settings' }))
    await waitFor(() => expect(router.state.location.pathname).toBe('/agents'))
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('opens a running agent in Chat straight from the Key and agent step', async () => {
    firstTime()
    const user = userEvent.setup()
    const { router } = renderApp('/')
    await guide()
    await user.click(screen.getByRole('button', { name: /get started/i }))
    await user.click(screen.getByRole('radio', { name: /product manager/i }))
    await user.click(screen.getByRole('button', { name: /continue/i }))
    // The seed user already has a model config, so Chat is on.
    const running = await screen.findByRole('list', { name: 'Agents running now' })
    expect(screen.queryByText(/Add your key above to chat/)).toBeNull()
    const [first] = within(running).getAllByRole('button', { name: /^Chat with / })
    await user.click(first!)
    await waitFor(() => expect(router.state.location.pathname).toBe('/chat'))
    expect(router.state.location.search).toMatchObject({ agent: expect.any(String) })
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('keeps Chat and Continue off until the user has a key', async () => {
    firstTime()
    server.use(
      http.get('*/api/llm-configs', () =>
        HttpResponse.json({ data: [], status_code: 200, message: 'ok' }),
      ),
    )
    const user = userEvent.setup()
    renderApp('/')
    await guide()
    await user.click(screen.getByRole('button', { name: /get started/i }))
    await user.click(screen.getByRole('radio', { name: /developer/i }))
    await user.click(screen.getByRole('button', { name: /continue/i }))
    const running = await screen.findByRole('list', { name: 'Agents running now' })
    expect(screen.getByText(/Add your key above to chat/)).toBeInTheDocument()
    for (const b of within(running).getAllByRole('button', { name: /^Chat with / })) {
      expect(b).toBeDisabled()
    }
    // Nothing past this step works without a key, so there is no skipping it.
    expect(screen.queryByRole('button', { name: 'Skip this step' })).toBeNull()
    expect(screen.getByRole('button', { name: /continue/i })).toBeDisabled()
    // A key connected here turns Chat and Continue on.
    await user.click(screen.getByRole('radio', { name: /anthropic/i }))
    await user.type(screen.getByLabelText('Anthropic API key'), 'sk-ant-test')
    await user.click(screen.getByRole('button', { name: 'Connect' }))
    await screen.findByText(/Connected · Anthropic/)
    expect(screen.queryByText(/Add your key above to chat/)).toBeNull()
    const again = screen.getByRole('list', { name: 'Agents running now' })
    for (const b of within(again).getAllByRole('button', { name: /^Chat with / })) {
      expect(b).toBeEnabled()
    }
    expect(screen.getByRole('button', { name: /continue/i })).toBeEnabled()
  })

  it("says where to get a key, linking the provider's keys page", async () => {
    firstTime()
    const user = userEvent.setup()
    renderApp('/')
    await guide()
    await user.click(screen.getByRole('button', { name: /get started/i }))
    await user.click(screen.getByRole('radio', { name: /developer/i }))
    await user.click(screen.getByRole('button', { name: /continue/i }))
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
    await user.click(screen.getByRole('button', { name: /get started/i }))
    await user.click(screen.getByRole('radio', { name: /developer/i }))
    await user.click(screen.getByRole('button', { name: /continue/i }))
    expect(await screen.findByRole('tab', { name: 'Upload a zip' })).toHaveAttribute(
      'aria-selected',
      'true',
    )
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
