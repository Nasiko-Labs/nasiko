/**
 * The LLM router page (plans/feat-llm-router.md §8, page tests): section order and states, the summary strip and its
 * filters, spend, configs (first config, edit, default, duplicate, delete), legacy warnings, the secrets rules and axe.
 * The agent sheet, keys and custom providers have their own files.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import axe from 'axe-core'
import { delay, http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, routerMockState } from '@/mocks/handlers'
import { configId } from '@/mocks/router'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequestBodies, recordRequests, server } from '@/test/setup'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, superuser: null, routerVariants: [] }))

const ready = async () => {
  await screen.findByRole('heading', { level: 1, name: copy.title })
  await screen.findByRole('table', { name: copy.agentsTitle })
  await waitFor(() => expect(screen.queryAllByText(copy.readingRouting)).toHaveLength(0))
}
const agentRows = () =>
  within(screen.getByRole('table', { name: copy.agentsTitle })).getAllByRole('row')
const rowFor = (name: string) => screen.getByRole('link', { name }).closest('tr') as HTMLElement
const strip = () => screen.getByRole('navigation', { name: copy.summaryLabel })
/** The summary tiles' names, which carry the strip's sentences ("5 attached"). */
const stripText = () =>
  within(strip())
    .getAllByRole('button')
    .map((b) => b.getAttribute('aria-label'))
    .join(' · ')
/** The page shows one tab at a time (Agents first). */
const showTab = async (name: string) => {
  const tab = await screen.findByRole('tab', { name })
  if (tab.getAttribute('aria-selected') !== 'true') await userEvent.click(tab)
}
/** Picks an option from a shadcn Select (it opens a listbox; there is no native select to target). */
const choose = async (combobox: HTMLElement, option: string) => {
  await userEvent.click(combobox)
  await userEvent.click(await screen.findByRole('option', { name: option }))
}
const configsList = () => screen.getByRole('list', { name: copy.configsTitle })
const configItem = (name: string) =>
  within(configsList()).getByText(name, { exact: true }).closest('li') as HTMLElement
const openMenu = async (name: string) => {
  await showTab(copy.anchors.configs)
  await userEvent.click(screen.getByRole('button', { name: copy.configActions(name) }))
}

describe('layout', () => {
  it('shows the tabs in order, with summary tiles that count the rows', async () => {
    renderApp('/router')
    await ready()
    expect(screen.getAllByRole('tab').map((t) => t.textContent)).toEqual([
      copy.anchors.agents,
      copy.anchors.configs,
      copy.anchors.providers,
      copy.anchors.classification,
    ])
    // One tab at a time: Your agents first.
    expect(screen.queryByRole('heading', { name: copy.configsTitle })).toBeNull()
    expect(stripText()).toContain('19 agents')
    expect(stripText()).toContain('14 on your default (anthropic, your key)')
    expect(stripText()).toContain('5 attached')
    expect(stripText()).toContain('0 no config (platform key)')
  })

  it('opens "How routing works" on a first visit with no configs and remembers a choice', async () => {
    configureMocks({ routerVariants: ['router-empty'] })
    renderApp('/router')
    const toggle = await screen.findByRole('button', { name: copy.howItWorks })
    await waitFor(() => expect(toggle).toHaveAttribute('aria-expanded', 'true'))
    await userEvent.click(toggle)
    expect(toggle).toHaveAttribute('aria-expanded', 'false')
    expect(localStorage.getItem('openruntime.router.howItWorks')).toBe('closed')
  })

  it('rows show source, the routing sentence and the key source', async () => {
    renderApp('/router')
    await ready()
    const research = rowFor('Research Agent')
    expect(research).toHaveTextContent(copy.source.attached)
    expect(research).toHaveTextContent(copy.overridden('gpt-4o'))
    expect(research).toHaveTextContent(copy.yourKey('OPENAI_API_KEY'))
    const reviewer = rowFor('Code Reviewer')
    expect(reviewer).toHaveTextContent(copy.pinnedConfig('gpt-4o-mini'))
    expect(reviewer).toHaveTextContent(copy.platformKey)
    expect(rowFor('Claude Code (admin@example.com)')).toHaveTextContent(copy.codingHarness)
  })

  it('reads routing at most 4 at a time', async () => {
    let inFlight = 0
    let peak = 0
    // Count, then fall through to the mock (a resolver that returns nothing passes the request on).
    server.use(
      http.get('/api/agents/:id/llm-config', async () => {
        inFlight++
        peak = Math.max(peak, inFlight)
        await delay(20)
        inFlight--
      }),
    )
    renderApp('/router')
    await ready()
    expect(peak).toBeGreaterThan(1)
    expect(peak).toBeLessThanOrEqual(4)
  })
})

describe('agents section states', () => {
  it('a failed row read shows Retry, and the strip says "at least"', async () => {
    const failing = seed.agents.find((a) => a.display_name === 'Doc Writer')!
    let fail = true
    server.use(
      http.get(`/api/agents/${failing.id}/llm-config`, () =>
        fail ? new HttpResponse('internal error', { status: 500 }) : undefined,
      ),
    )
    renderApp('/router')
    await screen.findByText(copy.couldntRead)
    await waitFor(() => expect(stripText()).toContain('at least 13 on your default'))
    fail = false
    server.resetHandlers()
    await userEvent.click(within(rowFor('Doc Writer')).getByRole('button', { name: copy.retry }))
    await waitFor(() => expect(stripText()).toContain('14 on your default'))
  })

  it('filter links narrow the rows and Show all clears them', async () => {
    const { router } = renderApp('/router')
    await ready()
    await userEvent.click(within(strip()).getByRole('button', { name: /5 attached/ }))
    expect(router.state.location.search).toEqual({ source: 'attached' })
    expect(agentRows()).toHaveLength(1 + 5)
    await userEvent.click(within(strip()).getByRole('button', { name: /no config/ }))
    expect(await screen.findByText(copy.noneInFilter)).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: copy.clearFilter }))
    await waitFor(() => expect(router.state.location.search).toEqual({}))
  })

  it('an unknown ?source falls back to all rows (the 14 on the default folded into one)', async () => {
    renderApp('/router?source=bogus')
    await ready()
    expect(agentRows()).toHaveLength(1 + 5 + 1)
    expect(document.querySelector('[data-folded]')).toHaveTextContent(copy.foldedTitle('14'))
  })

  it('no owned agents shows the empty state with a way to Agents', async () => {
    server.use(http.get('/api/agents', () => HttpResponse.json([])))
    renderApp('/router')
    expect(await screen.findByText(copy.noAgents)).toBeInTheDocument()
    expect(screen.getByRole('link', { name: copy.toAgents })).toHaveAttribute('href', '/agents')
  })
})

describe('spend', () => {
  it('shows 30-day router spend, "No routed calls" and the dropped-calls note', async () => {
    const calls = recordRequests()
    renderApp('/router')
    await ready()
    await waitFor(() =>
      expect(rowFor('Code Reviewer')).toHaveTextContent(/\$[\d.,]+ · [\d,]+ calls/),
    )
    expect(rowFor('Claude Code (admin@example.com)')).toHaveTextContent(copy.spendNone)
    expect(screen.getByText(/calls from deleted or other agents not shown/)).toBeInTheDocument()
    const usage = calls.urls.filter((u) => u.pathname === '/api/usage/by-agent')
    expect(usage).toHaveLength(1)
    expect(usage[0]!.searchParams.get('limit')).toBe('1000')
    expect(usage[0]!.searchParams.get('days')).toBe('30')
    calls.stop()
  })

  it('a full page says the list may be partial', async () => {
    configureMocks({ routerVariants: ['router-usage-full'] })
    renderApp('/router')
    await ready()
    expect(await screen.findByText(copy.partialNote)).toBeInTheDocument()
  })

  it('a failed spend read says so in every row, and the rest of the page works', async () => {
    configureMocks({ routerVariants: ['router-usage-fail'] })
    renderApp('/router')
    await ready()
    // Said once, visibly, under the table; each cell marks the gap (with the reason for screen readers).
    expect(await screen.findByText(copy.spendFailed, { selector: 'p' })).toBeInTheDocument()
    expect(rowFor('Code Reviewer')).toHaveTextContent(copy.spendFailed)
    expect(stripText()).toContain('14 on your default')
  })
})

describe('configs', () => {
  it('lists configs with the Default badge and Used by counts', async () => {
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.configs)
    const def = configItem('anthropic-default')
    expect(def).toHaveTextContent(copy.defaultBadge)
    await waitFor(() => expect(def).toHaveTextContent(copy.usedBy('15')))
    expect(configItem('research-tiers')).toHaveTextContent(copy.usedBy('2'))
  })

  it('the first config sends one POST with is_default and the strip counts it', async () => {
    configureMocks({ routerVariants: ['router-empty'] })
    const rec = recordRequestBodies()
    renderApp('/router')
    await showTab(copy.anchors.configs)
    await screen.findByText(copy.noConfigs)
    await userEvent.click(screen.getByRole('button', { name: copy.createFirst }))
    const sheet = await screen.findByRole('dialog', { name: copy.editorNew })
    await userEvent.type(within(sheet).getByLabelText(copy.fieldName), 'my-default')
    await choose(within(sheet).getByLabelText(copy.fieldProvider), 'anthropic')
    await userEvent.type(within(sheet).getByLabelText(copy.fieldModel), 'claude-sonnet-4')
    expect(within(sheet).getByRole('checkbox', { name: copy.useAsDefault })).toBeChecked()
    const save = within(sheet).getByRole('button', { name: copy.save })
    await userEvent.dblClick(save)
    // It affects every agent, so the sheet closes (no follow-through panel).
    await waitFor(() => expect(screen.queryByRole('dialog', { name: copy.editorNew })).toBeNull())
    await rec.flush()
    const posts = rec.requests.filter(
      (r) => r.method === 'POST' && r.url.pathname === '/api/llm-configs',
    )
    expect(posts).toHaveLength(1)
    expect(posts[0]!.body).toMatchObject({
      name: 'my-default',
      provider: 'anthropic',
      model: 'claude-sonnet-4',
      is_default: true,
    })
    await waitFor(() =>
      expect(stripText()).toContain('19 on your default (anthropic, platform key)'),
    )
  })

  it('edit sends only changed fields and shows Affects N', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.configs)
    await openMenu('research-tiers')
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.edit }))
    const sheet = await screen.findByRole('dialog', { name: copy.editorEdit('research-tiers') })
    expect(within(sheet).getByText((t) => t.startsWith(copy.affects('2')))).toBeInTheDocument()
    // Advanced is a disclosure; open it when the field is hidden.
    if (!within(sheet).queryByLabelText(copy.temperature))
      await userEvent.click(within(sheet).getByRole('button', { name: copy.advanced }))
    const temp = within(sheet).getByLabelText(copy.temperature)
    await userEvent.clear(temp)
    await userEvent.type(temp, '0.7')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await waitFor(() => expect(sheet).not.toBeInTheDocument())
    await rec.flush()
    const patch = rec.requests.find(
      (r) => r.method === 'PATCH' && r.url.pathname === `/api/llm-configs/${configId(2)}`,
    )!
    expect(patch.body).toEqual({ temperature: 0.7 })
  })

  it('removing a fallback sends an empty list', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.configs)
    await openMenu('anthropic-default')
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.edit }))
    const sheet = await screen.findByRole('dialog', { name: copy.editorEdit('anthropic-default') })
    await userEvent.click(
      within(sheet).getByRole('button', { name: copy.removeFallback('openai/gpt-4o-mini') }),
    )
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await waitFor(() => expect(sheet).not.toBeInTheDocument())
    await rec.flush()
    expect(rec.requests.find((r) => r.method === 'PATCH')!.body).toEqual({ fallback_models: [] })
  })

  it('set and remove the default', async () => {
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.configs)
    await openMenu('research-tiers')
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.setDefault }))
    await waitFor(() => expect(configItem('research-tiers')).toHaveTextContent(copy.defaultBadge))
    expect(configItem('anthropic-default')).not.toHaveTextContent(copy.defaultBadge)
    await openMenu('research-tiers')
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.removeDefault }))
    await waitFor(() =>
      expect(configItem('research-tiers')).not.toHaveTextContent(copy.defaultBadge),
    )
    await waitFor(() => expect(stripText()).toContain('14 no config (platform key)'))
  })

  it('duplicate prefills a unique name and saves a new config', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.configs)
    await openMenu('fast-openai')
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.duplicate }))
    const sheet = await screen.findByRole('dialog', { name: copy.editorDuplicate('fast-openai') })
    expect(within(sheet).getByLabelText(copy.fieldName)).toHaveValue('fast-openai copy')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await within(sheet).findByText(copy.duplicateSaved('1'))
    await rec.flush()
    const post = rec.requests.find(
      (r) => r.method === 'POST' && r.url.pathname === '/api/llm-configs',
    )!
    expect(post.body).toMatchObject({
      name: 'fast-openai copy',
      provider: 'openai',
      pinned: true,
      pinned_model: 'gpt-4o-mini',
    })
    expect(post.body).not.toHaveProperty('is_default', true)
  })

  it('deletes an unused config', async () => {
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.configs)
    // Detach both research-tiers agents first through the mock, so this one is unused.
    for (const [id, r] of routerMockState().routing)
      if (r.llm_config_id === configId(2))
        routerMockState().routing.set(id, { ...r, llm_config_id: null, pinned_model: null })
    await openMenu('research-tiers')
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.delete }))
    const dialog = await screen.findByRole('alertdialog')
    await userEvent.click(within(dialog).getByRole('button', { name: copy.delete }))
    await waitFor(() => expect(within(configsList()).queryByText('research-tiers')).toBeNull())
  })

  it('an in-use delete lists the attached agents with Change routing', async () => {
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.configs)
    await openMenu('research-tiers')
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.delete }))
    const dialog = await screen.findByRole('alertdialog')
    await userEvent.click(within(dialog).getByRole('button', { name: copy.delete }))
    expect(await within(dialog).findByText(copy.deleteInUse('2'))).toBeInTheDocument()
    expect(within(dialog).getByText('Support Bot')).toBeInTheDocument()
    expect(within(dialog).getByText('Research Agent')).toBeInTheDocument()
    await userEvent.click(within(dialog).getAllByRole('button', { name: copy.changeRouting })[0]!)
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull())
    expect(await screen.findByRole('dialog', { name: /Change routing · / })).toBeInTheDocument()
  })

  it('deleting the default warns what it moves', async () => {
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.configs)
    await openMenu('anthropic-default')
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.delete }))
    const dialog = await screen.findByRole('alertdialog')
    expect(dialog).toHaveTextContent(copy.affectsDeleteDefault('14'))
  })

  it('each router-legacy config shows its warning', async () => {
    configureMocks({ routerVariants: ['router-legacy'] })
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.configs)
    await waitFor(() =>
      expect(configItem('cli-openrouter')).toHaveTextContent(copy.openrouterHidden),
    )
    expect(configItem('lost-key')).toHaveTextContent(copy.missingKey('DELETED_KEY'))
    expect(configItem('old-endpoint')).toHaveTextContent(copy.providerGone)
    expect(configItem('off-catalog')).toHaveTextContent(copy.notInCatalog)
  })

  it('a failed configs read shows the section error and the agents still render', async () => {
    server.use(
      http.get('/api/llm-configs', () => new HttpResponse('internal error', { status: 500 })),
    )
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.configs)
    const configs = screen.getByRole('heading', { name: copy.configsTitle }).closest('section')!
    expect(await within(configs).findByRole('button', { name: copy.retry })).toBeInTheDocument()
  })
})

describe('providers', () => {
  it('lists the catalog, marks google as not routable and shows the registry', async () => {
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.providers)
    const providers = screen.getByRole('heading', { name: copy.providersTitle }).closest('section')!
    expect(within(providers).getByText(copy.notRoutable('google'))).toBeInTheDocument()
    expect(within(providers).getByText(copy.registryTitle)).toBeInTheDocument()
    expect(within(providers).getByText(copy.registryWayOut)).toBeInTheDocument()
  })

  it('a failed catalog read shows its error without breaking the page', async () => {
    configureMocks({ routerVariants: ['router-catalog-fail'] })
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.providers)
    expect(await screen.findByText(copy.catalogFailed)).toBeInTheDocument()
  })
})

describe('never', () => {
  it('reads a secret by name', async () => {
    const calls = recordRequests()
    renderApp('/router')
    await ready()
    await openMenu('anthropic-default')
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.edit }))
    await screen.findByRole('dialog', { name: copy.editorEdit('anthropic-default') })
    expect(calls.urls.filter((u) => /^\/api\/secrets\/.+/.test(u.pathname))).toEqual([])
    calls.stop()
  })
})

describe('axe', () => {
  const check = async () => {
    const result = await axe.run(document.body, {
      rules: { 'color-contrast': { enabled: false }, region: { enabled: false } },
    })
    expect(
      result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`),
    ).toEqual([])
  }
  it('the page', async () => {
    renderApp('/router')
    await ready()
    await check()
  })
  it('the routing sheet, the custom provider sheet and the delete dialog', async () => {
    renderApp('/router')
    await ready()
    await userEvent.click(
      screen.getByRole('button', { name: `${copy.changeRouting}: Research Agent` }),
    )
    await screen.findByRole('radio', { name: copy.useMyDefault })
    await check()
    await userEvent.keyboard('{Escape}')
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    await showTab(copy.anchors.providers)
    await userEvent.click(screen.getByRole('button', { name: copy.addCustom }))
    await screen.findByRole('dialog', { name: copy.cpNew })
    await check()
    await userEvent.keyboard('{Escape}')
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    await openMenu('research-tiers')
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.delete }))
    await screen.findByRole('alertdialog')
    await check()
  })
  it('the config editor', async () => {
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.configs)
    await userEvent.click(screen.getByRole('button', { name: copy.newConfig }))
    await screen.findByRole('dialog', { name: copy.editorNew })
    await check()
  })
})

describe('review fixes', () => {
  it('a 409 count that differs from the listed agents re-reads their routing', async () => {
    configureMocks({ routerVariants: ['router-409'] })
    const calls = recordRequests()
    renderApp('/router')
    await ready()
    const before = calls.urls.filter((u) => u.pathname.endsWith('/llm-config')).length
    await openMenu('research-tiers')
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.delete }))
    const dialog = await screen.findByRole('alertdialog')
    await userEvent.click(within(dialog).getByRole('button', { name: copy.delete }))
    expect(await within(dialog).findByText(copy.deleteInUse('3'))).toBeInTheDocument()
    await waitFor(() =>
      expect(calls.urls.filter((u) => u.pathname.endsWith('/llm-config')).length).toBeGreaterThan(
        before,
      ),
    )
    calls.stop()
  })

  it('agent rows link their config to its row in Your configs', async () => {
    renderApp('/router')
    await ready()
    await userEvent.click(
      within(rowFor('Code Reviewer')).getByRole('button', { name: 'fast-openai' }),
    )
    await waitFor(() => expect(configItem('fast-openai')).toHaveFocus())
  })

  it('an empty or failed model registry says so', async () => {
    server.use(
      http.get('/api/model-registry', () =>
        HttpResponse.json({ data: [], status_code: 200, message: 'ok' }),
      ),
    )
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.providers)
    expect(await screen.findByText(copy.registryEmpty)).toBeInTheDocument()
  })

  it('a default changed in another tab re-reads the rows that use it', async () => {
    const calls = recordRequests()
    const { queryClient } = renderApp('/router')
    await ready()
    const before = calls.urls.filter((u) => u.pathname.endsWith('/llm-config')).length
    const s = routerMockState()
    for (const c of s.configs) c.is_default = c.id === configId(2)
    s.configs.find((c) => c.id === configId(2))!.updated_at = new Date(now() + 60_000).toISOString()
    s.configs.find((c) => c.id === configId(1))!.updated_at = new Date(now() + 60_000).toISOString()
    await queryClient.refetchQueries({ queryKey: ['router', 'configs'] })
    await waitFor(() => expect(stripText()).toContain('on your default (openai, your key)'))
    expect(calls.urls.filter((u) => u.pathname.endsWith('/llm-config')).length).toBeGreaterThan(
      before,
    )
    calls.stop()
  })
})

// Regression: ISSUE-002 (/qa 2026-09-28, .gstack/qa-reports/qa-report-localhost-2026-09-28.md): a default count
// with no default config.
describe('How routing works', () => {
  it('with no default config, it does not count agents on it', async () => {
    configureMocks({ routerVariants: ['router-empty'] })
    renderApp('/router')
    await screen.findByRole('table', { name: copy.agentsTitle })
    await waitFor(() => expect(screen.queryAllByText(copy.readingRouting)).toHaveLength(0))
    const how = await screen.findByRole('region', { name: copy.howTitle })
    expect(how).toHaveTextContent(copy.howSteps[0]!.title)
    expect(how).not.toHaveTextContent(/of your 19 agents use your default/)
  })
})
