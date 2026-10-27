/**
 * The desktop app's LLM routing card on Harnesses (routing.ts, components/RoutingCard.tsx): `nasiko connect <harness>
 * [--config]` from the page. Hidden in a browser; in the app, one row per routable harness, opt-in, with the config
 * to route with, take-over of a CLI connection, and Disconnect.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, type MockVariant } from '@nasiko/mocks/handlers'
import { generateHarnessSeed } from '@nasiko/mocks/seed-harness'
import { FIXED, now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { renderApp } from '@nasiko/testing/renderApp'
import type { LlmConfig } from '@nasiko/features/router/types'
import { recordRequestBodies, recordRequests, server } from '@nasiko/testing/setup'
import {
  attachedHarnesses,
  configModel,
  connectAction,
  initialChoice,
  type RoutedHarness,
} from './routing'

const harnessSeed = generateHarnessSeed({ anchor: FIXED })
const ROUTING = '/__nasiko/coding-agents/routing'

setupPinnedSeed()

function as(variant: MockVariant | null = null) {
  configureMocks({ seed, now, loggedIn: true, harnessSeed, persona: 'admin', variant })
}
afterEach(() =>
  configureMocks({ seed, now, loggedIn: true, harnessSeed, persona: null, variant: null }),
)

const card = () => screen.findByTestId('harness-routing')
const row = async (id: string) => {
  const c = await card()
  await waitFor(() => expect(c.querySelector(`[data-harness="${id}"]`)).not.toBeNull())
  return c.querySelector<HTMLElement>(`[data-harness="${id}"]`)!
}

describe('in a browser', () => {
  it('shows no routing card and never asks the desktop for it', async () => {
    as()
    const rec = recordRequests()
    try {
      renderApp('/harnesses')
      await screen.findByTestId('harness-summary')
      expect(screen.queryByTestId('harness-routing')).toBeNull()
      expect(rec.urls.some((u) => u.pathname === ROUTING)).toBe(false)
    } finally {
      rec.stop()
    }
  })
})

describe('in the desktop app', () => {
  it('lists each harness: not routed, routed by the CLI, not on this computer', async () => {
    as('desktop-unasked')
    renderApp('/harnesses')
    const claude = await row('claude')
    expect(within(claude).getByText('Not routed through Nasiko')).toBeInTheDocument()
    expect(within(claude).getByRole('button', { name: 'Connect' })).toBeEnabled()
    expect(within(claude).queryByRole('button', { name: 'Disconnect' })).toBeNull()
    // Nothing is routed until asked: the picker starts on the user's default config.
    expect(
      within(claude).getByRole('combobox', { name: 'LLM config for Claude Code' }),
    ).toHaveTextContent('anthropic-default (your default) · anthropic/claude-sonnet-4')

    const codex = await row('codex')
    expect(within(codex).getByText(/Connected with the Nasiko CLI/)).toBeInTheDocument()
    expect(within(codex).getByRole('button', { name: 'Take over' })).toBeInTheDocument()
    expect(within(codex).getByRole('button', { name: 'Disconnect' })).toBeInTheDocument()

    const opencode = await row('opencode')
    expect(within(opencode).getByText('Not on this computer')).toBeInTheDocument()
    expect(within(opencode).queryByRole('button')).toBeNull()
    expect(within(opencode).getByRole('combobox')).toBeDisabled()
  })

  it('connects with the default config, then disconnects', async () => {
    as('desktop-unasked')
    const user = userEvent.setup()
    const rec = recordRequestBodies()
    try {
      renderApp('/harnesses')
      const claude = await row('claude')
      await user.click(within(claude).getByRole('button', { name: 'Connect' }))
      await within(claude).findByText('Restart Claude Code so it picks up the change.')
      expect(within(claude).getByRole('button', { name: 'Disconnect' })).toBeInTheDocument()
      // Connected with what it was connected with: nothing new to apply.
      expect(within(claude).queryByRole('button', { name: /Connect|Use this config/ })).toBeNull()
      const connect = rec.requests.find((r) => r.url.pathname === `${ROUTING}/connect`)
      // The default config is attached by id, never left to follow the default, so the harness counts for
      // Token optimization.
      const body = connect?.body as { agent: string; config: string }
      expect(body.agent).toBe('claude')
      expect(body.config).toMatch(/^5eed0007-/)

      await user.click(within(claude).getByRole('button', { name: 'Disconnect' }))
      await within(claude).findByText('Not routed through Nasiko')
      const disconnect = rec.requests.find((r) => r.url.pathname === `${ROUTING}/disconnect`)
      expect(disconnect?.body).toEqual({ agent: 'claude' })
    } finally {
      rec.stop()
    }
  })

  it('connects with a chosen config, and takes a harness over from the CLI', async () => {
    as('desktop-unasked')
    const user = userEvent.setup()
    const rec = recordRequestBodies()
    try {
      renderApp('/harnesses')
      const codex = await row('codex')
      await user.click(within(codex).getByRole('combobox', { name: 'LLM config for Codex' }))
      await user.click(await screen.findByRole('option', { name: /^fast-openai/ }))
      await user.click(within(codex).getByRole('button', { name: 'Take over' }))
      await within(codex).findByText(/Took over Codex from the Nasiko CLI/)
      expect(within(codex).queryByText(/Connected with the Nasiko CLI/)).toBeNull()
      const connect = rec.requests.find((r) => r.url.pathname === `${ROUTING}/connect`)
      const body = connect?.body as { agent: string; config: string }
      expect(body.agent).toBe('codex')
      expect(body.config).toMatch(/^5eed0007-/)
    } finally {
      rec.stop()
    }
  })

  it("says why a connect was refused, in the desktop's own words", async () => {
    as('desktop-unasked')
    server.use(
      http.post(`${ROUTING}/connect`, () =>
        HttpResponse.json(
          { message: "Nasiko can't route Codex with this LLM config: unknown provider 'x'." },
          { status: 422 },
        ),
      ),
    )
    const user = userEvent.setup()
    renderApp('/harnesses')
    const claude = await row('claude')
    await user.click(within(claude).getByRole('button', { name: 'Connect' }))
    const alert = await within(claude).findByRole('alert')
    expect(alert).toHaveTextContent(
      "Couldn't change Claude Code's routing: Nasiko can't route Codex with this LLM config: unknown provider 'x'.",
    )
    expect(within(claude).getByText('Not routed through Nasiko')).toBeInTheDocument()
  })
})

describe('pure rules', () => {
  const harness = (over: Partial<RoutedHarness> = {}): RoutedHarness => ({
    id: 'claude',
    name: 'Claude Code',
    detected: true,
    connected_by: null,
    agent_id: 'a',
    agent_name: 'n',
    server_url: null,
    mcp: true,
    changed: false,
    ...over,
  })
  const attached = {
    agent_id: 'a',
    llm_config_id: 'c1',
    llm_config: null,
    source: 'attached' as const,
    inbound_format: 'openai',
    pinned_model: null,
  }

  const config = (id: string, isDefault = false) => ({ id, is_default: isDefault }) as LlmConfig

  it('starts on the attached config, else the default, else the first', () => {
    const configs = [config('c2'), config('c3', true)]
    expect(initialChoice(attached, configs)).toBe('c1')
    expect(initialChoice(undefined, configs)).toBe('c3')
    expect(initialChoice({ ...attached, source: 'owner-default' }, configs)).toBe('c3')
    expect(initialChoice(undefined, [config('c2')])).toBe('c2')
    expect(initialChoice(undefined, [])).toBeNull()
  })

  it('counts only harnesses with an LLM config attached for Token optimization', () => {
    const rows = ['a', 'b', 'c', 'd']
    const { included, loading } = attachedHarnesses(rows, [
      { state: 'ok', routing: attached },
      { state: 'ok', routing: { ...attached, llm_config_id: null, source: 'owner-default' } },
      { state: 'error' },
      { state: 'pending' },
    ])
    expect(included).toEqual(['a'])
    expect(loading).toBe(true)
    expect(attachedHarnesses(['a'], [{ state: 'ok', routing: attached }]).loading).toBe(false)
  })

  it('offers a reconnect only when something would change', () => {
    expect(connectAction(harness(), 'c1', undefined)).toBe('connect')
    expect(connectAction(harness(), null, undefined)).toBeNull()
    expect(connectAction(harness({ connected_by: 'cli' }), 'c1', attached)).toBe('take-over')
    expect(connectAction(harness({ connected_by: 'desktop' }), 'c1', attached)).toBeNull()
    expect(connectAction(harness({ connected_by: 'desktop' }), 'c2', attached)).toBe('apply')
    expect(connectAction(harness({ connected_by: 'desktop', changed: true }), 'c1', attached)).toBe(
      'apply',
    )
    expect(connectAction(harness({ detected: false }), 'c1', undefined)).toBeNull()
  })

  it("names a config's model as the router would pick it", () => {
    const tiers = { model: null, tier1_model: 'a', tier2_model: 'b', tier3_model: null }
    expect(configModel(tiers)).toBe('b')
    expect(configModel({ ...tiers, model: 'm' })).toBe('m')
  })
})
