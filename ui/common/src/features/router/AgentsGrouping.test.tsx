/**
 * Your agents folding and search, and "How routing works" (plans/feat-llm-router.md §4.1-§4.2 amendment, 2026-09-28):
 * agents that follow your default fold into one row (remembered per viewer), filters and searches unfold, a row
 * changed on the page stays visible, and the polished explainer opens, closes and links to its numbers.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import axe from 'axe-core'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, routerVariants: [] }))

const ready = async () => {
  await screen.findByRole('table', { name: copy.agentsTitle })
  await waitFor(() => expect(screen.queryAllByText(copy.readingRouting)).toHaveLength(0))
}
const agentsTable = () => screen.getByRole('table', { name: copy.agentsTitle })
const folded = () => document.querySelector('[data-folded]') as HTMLElement | null
const toggle = () => within(folded()!).getByRole('button')

describe('Your agents folding', () => {
  it('folds the 14 agents on your default into one row with their config, key and spend', async () => {
    renderApp('/router')
    await ready()
    expect(folded()).toHaveTextContent(copy.foldedTitle('14'))
    expect(folded()).toHaveTextContent(
      copy.foldedDetail('anthropic-default', copy.yourKey('ANTHROPIC_API_KEY')),
    )
    await waitFor(() => expect(folded()).toHaveTextContent(/\$[\d.,]+ · [\d,]+ calls in 30 days/))
    expect(toggle()).toHaveAttribute('aria-expanded', 'false')
    expect(within(agentsTable()).queryByRole('link', { name: 'Doc Writer' })).toBeNull()
    // The ones worth a look stay: attached and overridden agents.
    expect(within(agentsTable()).getByRole('link', { name: 'Research Agent' })).toBeInTheDocument()
  })

  it('Show all opens the group and the choice is remembered', async () => {
    renderApp('/router')
    await ready()
    await userEvent.click(toggle())
    expect(toggle()).toHaveAttribute('aria-expanded', 'true')
    expect(toggle()).toHaveTextContent(copy.foldedHide)
    expect(within(agentsTable()).getByRole('link', { name: 'Doc Writer' })).toBeInTheDocument()
    expect(localStorage.getItem('openruntime.router.defaultsOpen')).toBe('open')
  })

  it('the "on your default" filter lists those agents unfolded', async () => {
    renderApp('/router?source=default')
    await ready()
    expect(folded()).toBeNull()
    expect(within(agentsTable()).getAllByRole('row')).toHaveLength(1 + 14)
  })

  it('an agent moved onto the default on this page stays visible after the save', async () => {
    renderApp('/router')
    await ready()
    await userEvent.click(
      screen.getByRole('button', { name: `${copy.changeRouting}: Code Reviewer` }),
    )
    const sheet = await screen.findByRole('dialog', { name: copy.routingTitle('Code Reviewer') })
    await userEvent.click(await within(sheet).findByRole('radio', { name: copy.useMyDefault }))
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await within(sheet).findByText(copy.savedPlan('Code Reviewer'))
    await userEvent.click(within(sheet).getByRole('button', { name: copy.done }))
    const reviewer = await within(agentsTable()).findByRole('link', { name: 'Code Reviewer' })
    await waitFor(() => expect(reviewer.closest('tr')).toHaveTextContent(copy.source.default))
    // It shows above the group, so the group says "more" and agrees with the strip's 15.
    expect(folded()).toHaveTextContent(copy.foldedTitleMore('14'))
    expect(
      within(screen.getByRole('navigation', { name: copy.summaryLabel })).getByRole('button', {
        name: /^15 on your default/,
      }),
    ).toBeInTheDocument()
  })

  it('a search appears from 20 agents, matches names, and unfolds while it narrows', async () => {
    const base = seed.agents.find((a) => !a.deleted)!
    const many = Array.from({ length: 22 }, (_, i) => ({
      ...base,
      id: `0000feed-0000-4000-8000-${String(i).padStart(12, '0')}`,
      name: `extra-${i}`,
      display_name: i === 7 ? 'Needle Agent' : `Extra ${i}`,
      owner_id: '5eed0000-0000-4000-8000-00000000a001',
      status: 'running',
      tags: [],
      skills: [],
    }))
    server.use(
      http.get('/api/agents', ({ request }) =>
        new URL(request.url).searchParams.get('offset') === '0'
          ? HttpResponse.json(many)
          : HttpResponse.json([]),
      ),
      http.get('/api/agents/:id/llm-config', ({ params }) =>
        HttpResponse.json({
          data: {
            agent_id: String(params.id),
            llm_config_id: null,
            llm_config: null,
            source: 'owner-default',
            inbound_format: 'openai',
            pinned_model: null,
          },
          status_code: 200,
          message: 'ok',
        }),
      ),
    )
    renderApp('/router')
    const search = await screen.findByRole('searchbox', { name: copy.searchAgents })
    await ready()
    expect(folded()).toHaveTextContent(copy.foldedTitle('22'))
    await userEvent.type(search, 'needle')
    expect(folded()).toBeNull()
    expect(within(agentsTable()).getAllByRole('row')).toHaveLength(2)
    expect(within(agentsTable()).getByRole('link', { name: 'Needle Agent' })).toBeInTheDocument()
    await userEvent.clear(search)
    await userEvent.type(search, 'zzz')
    expect(screen.getByText(copy.searchNone('zzz'))).toBeInTheDocument()
  })

  it('a routing change from inside the open group returns focus to that agent’s button in its new place', async () => {
    renderApp('/router')
    await ready()
    await userEvent.click(toggle())
    const name = `${copy.changeRouting}: Doc Writer`
    await userEvent.click(within(agentsTable()).getByRole('button', { name }))
    const sheet = await screen.findByRole('dialog', { name: copy.routingTitle('Doc Writer') })
    await userEvent.type(await within(sheet).findByLabelText(copy.overrideModel), 'gpt-4o')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await within(sheet).findByText(copy.savedPlan('Doc Writer'))
    await userEvent.click(within(sheet).getByRole('button', { name: copy.done }))
    await waitFor(() =>
      expect(document.activeElement).toBe(within(agentsTable()).getByRole('button', { name })),
    )
  })

  it('filtering with the group open never shows an agent twice', async () => {
    renderApp('/router')
    await ready()
    await userEvent.click(toggle())
    await userEvent.click(
      within(screen.getByRole('navigation', { name: copy.summaryLabel })).getByRole('button', {
        name: /on your default/,
      }),
    )
    await waitFor(() => expect(folded()).toBeNull())
    expect(within(agentsTable()).getAllByRole('link', { name: 'Doc Writer' })).toHaveLength(1)
  })

  it('under 20 agents there is no search', async () => {
    renderApp('/router')
    await ready()
    expect(screen.queryByRole('searchbox', { name: copy.searchAgents })).toBeNull()
  })
})

describe('How routing works', () => {
  it('shows five numbered steps, your numbers, and closes from its own button (remembered)', async () => {
    renderApp('/router')
    await ready()
    await userEvent.click(screen.getByRole('button', { name: copy.howItWorks }))
    const how = await screen.findByRole('region', { name: copy.howTitle })
    const steps = within(how).getAllByRole('listitem')
    expect(steps).toHaveLength(5)
    for (const [i, s] of copy.howSteps.entries()) {
      expect(steps[i]).toHaveTextContent(String(i + 1))
      expect(steps[i]).toHaveTextContent(s.title)
      expect(steps[i]).toHaveTextContent(s.detail)
    }
    expect(how).toHaveTextContent(copy.howCounts('14', '19', 'anthropic-default'))
    await userEvent.click(within(how).getByRole('button', { name: copy.howClose }))
    await waitFor(() => expect(screen.queryByRole('region', { name: copy.howTitle })).toBeNull())
    expect(screen.getByRole('button', { name: copy.howItWorks })).toHaveFocus()
    expect(localStorage.getItem('openruntime.router.howItWorks')).toBe('closed')
    expect(screen.getByRole('button', { name: copy.howItWorks })).toHaveAttribute(
      'aria-expanded',
      'false',
    )
  })

  it('"Show them" filters Your agents to the ones on your default', async () => {
    const { router } = renderApp('/router')
    await ready()
    await userEvent.click(screen.getByRole('button', { name: copy.howItWorks }))
    await userEvent.click(await screen.findByRole('button', { name: copy.howShowDefaults }))
    await waitFor(() => expect(router.state.location.search).toEqual({ source: 'default' }))
    expect(folded()).toBeNull()
  })

  it('axe: the explainer open and the folded agents', async () => {
    renderApp('/router')
    await ready()
    await userEvent.click(screen.getByRole('button', { name: copy.howItWorks }))
    await screen.findByRole('region', { name: copy.howTitle })
    const result = await axe.run(document.body, {
      rules: { 'color-contrast': { enabled: false }, region: { enabled: false } },
    })
    expect(
      result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`),
    ).toEqual([])
  })
})
