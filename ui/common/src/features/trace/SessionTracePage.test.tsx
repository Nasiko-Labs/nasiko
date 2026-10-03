import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest'
import { copy } from '@/features/observability/copy'
import { configureMocks } from '@/mocks/handlers'
import {
  generateSpans,
  observabilityData,
  sessionDetail,
  SHOWCASE_SESSION,
} from '@/mocks/observability'
import { seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'

setupPinnedSeed()
afterEach(() => configureMocks({ variant: null }))

// Radix Select needs pointer-capture APIs jsdom doesn't have (same stub as the TokenOps filter tests).
const proto = Element.prototype as unknown as Record<string, unknown>
const stubbed = [
  'hasPointerCapture',
  'releasePointerCapture',
  'setPointerCapture',
  'scrollIntoView',
].filter((k) => !(k in proto))
beforeAll(() => {
  for (const k of stubbed) proto[k] = k === 'hasPointerCapture' ? () => false : () => {}
})
afterAll(() => {
  for (const k of stubbed) delete proto[k]
})

const data = observabilityData(seed)
const showcaseSpans = () => generateSpans(seed, data.showcaseTraceId)

describe('Session trace', () => {
  it('deep link: ?trace and ?span (hex) select that span and open the span panel', async () => {
    const plan = showcaseSpans().find((s) => s.name === 'llm.plan')!
    renderApp(`/sessions/${SHOWCASE_SESSION}?trace=${data.showcaseTraceId}&span=${plan.hex}`)
    const tree = await screen.findByRole('tree', { name: 'Spans' }, { timeout: 8000 })
    expect(within(tree).getByRole('treeitem', { selected: true })).toHaveAccessibleName(
      /^llm\.plan, LLM, /,
    )
    const panel = await screen.findByRole('region', { name: 'llm.plan' })
    // Per-span cost comes from SpanDetail.
    expect(await within(panel).findByText(/^\$\d+\.\d\d$|^<\$0\.01$/)).toBeInTheDocument()
  })

  it('an unknown span falls back to the default span with a notice', async () => {
    renderApp(`/sessions/${SHOWCASE_SESSION}?trace=${data.showcaseTraceId}&span=deadbeefdeadbeef`)
    expect(await screen.findByText(copy.unknownSpan, {}, { timeout: 8000 })).toBeInTheDocument()
    const tree = screen.getByRole('tree', { name: 'Spans' })
    expect(within(tree).getByRole('treeitem', { selected: true })).toHaveAccessibleName(
      /attempt 6 of 6/,
    )
  })

  it('span panel tabs: prompt and response render as text; attributes filter', async () => {
    const user = userEvent.setup()
    const plan = showcaseSpans().find((s) => s.name === 'llm.plan')!
    renderApp(`/sessions/${SHOWCASE_SESSION}?trace=${data.showcaseTraceId}&span=${plan.hex}`)
    await user.click(
      await screen.findByRole('tab', { name: 'Prompt & response' }, { timeout: 8000 }),
    )
    expect(
      await within(screen.getByRole('tabpanel')).findByText(/^Review PR #481/),
    ).toBeInTheDocument()
    await user.click(screen.getByRole('tab', { name: 'Attributes' }))
    await user.type(screen.getByRole('searchbox', { name: 'Filter attributes' }), 'model')
    // Keys are grouped by namespace; each row keeps its full key as the title.
    expect(await screen.findByTitle('gen_ai.request.model')).toHaveTextContent('request.model')
    expect(screen.queryByTitle('gen_ai.operation.name')).toBeNull()
  })

  it("the summary names the called agent from the span detail's nested agent.id (as the server un-flattens it)", async () => {
    const user = userEvent.setup()
    renderApp(`/sessions/${SHOWCASE_SESSION}?trace=${data.showcaseTraceId}`)
    const summary = await screen.findByRole('region', { name: 'What happened' }, { timeout: 8000 })
    // The cascade clause sits under Details when the first three sentences are taken.
    await user.click(await within(summary).findByRole('button', { name: 'Details' }))
    await waitFor(
      () => expect(summary).toHaveTextContent(/called QA Tester \d+ times? through the proxy/),
      { timeout: 8000 },
    )
  })

  it('keyboard: arrow keys move through the tree, Enter selects, ←/→ close and open a group', async () => {
    const user = userEvent.setup()
    renderApp(`/sessions/${SHOWCASE_SESSION}?trace=${data.showcaseTraceId}`)
    const tree = await screen.findByRole('tree', { name: 'Spans' }, { timeout: 8000 })
    const selected = within(tree).getByRole('treeitem', { selected: true })
    selected.focus()
    await user.keyboard('{Home}')
    expect(document.activeElement).toHaveAccessibleName(/^planner, Planner, /)
    await user.keyboard('{ArrowDown}{Enter}')
    await waitFor(() =>
      expect(within(tree).getByRole('treeitem', { selected: true })).toHaveAccessibleName(
        /^llm\.plan, /,
      ),
    )
    // The retry group was open only because the selection was inside it.
    const group = within(tree).getByRole('treeitem', { name: /^tool\.get_diff ×6, 6 failed/ })
    expect(group).toHaveAttribute('aria-expanded', 'false')
    group.focus()
    await user.keyboard('{ArrowRight}')
    await waitFor(() =>
      expect(within(tree).getByRole('treeitem', { name: /^tool\.get_diff ×6/ })).toHaveAttribute(
        'aria-expanded',
        'true',
      ),
    )
    within(tree)
      .getByRole('treeitem', { name: /^tool\.get_diff ×6/ })
      .focus()
    await user.keyboard('{ArrowLeft}')
    await waitFor(() =>
      expect(within(tree).getByRole('treeitem', { name: /^tool\.get_diff ×6/ })).toHaveAttribute(
        'aria-expanded',
        'false',
      ),
    )
  })

  it('the Table view lists the spans in start order', async () => {
    const user = userEvent.setup()
    renderApp(`/sessions/${SHOWCASE_SESSION}?trace=${data.showcaseTraceId}`)
    await user.click(await screen.findByRole('button', { name: /Table/ }, { timeout: 8000 }))
    const table = screen.getByRole('table', { name: 'Spans in start order' })
    const names = within(table)
      .getAllByRole('row')
      .slice(1)
      .map((r) => r.querySelector('td button')?.textContent)
    expect(names[0]).toBe('planner')
    expect(names).toHaveLength(showcaseSpans().length)
  })

  it('a session with many traces offers a trace picker; switching trace clears the span', async () => {
    const user = userEvent.setup()
    const { router } = renderApp(
      `/sessions/${SHOWCASE_SESSION}?trace=${data.showcaseTraceId}&span=${showcaseSpans()[0].hex}`,
    )
    const picker = await screen.findByRole(
      'combobox',
      { name: /^Trace \(\d+ in this session\)$/ },
      { timeout: 8000 },
    )
    await user.click(picker)
    const options = await screen.findAllByRole('option')
    await user.click(
      options.find(
        (o) => !o.textContent?.includes('largest') && !o.textContent?.includes('failing'),
      )!,
    )
    await waitFor(() => expect(router.state.location.search).not.toHaveProperty('span'))
  })

  it('states: 404 session, not configured (503), trace store error (500)', async () => {
    renderApp('/sessions/nope')
    expect(await screen.findByText(copy.notFoundSession)).toBeInTheDocument()
    screen.getByText(copy.notFoundSession).closest('div')!.remove()
    configureMocks({ variant: 'trace-503' })
    renderApp(`/sessions/${SHOWCASE_SESSION}`)
    expect(await screen.findByText(copy.notConfigured)).toBeInTheDocument()
    expect(screen.getByText(copy.notConfiguredFix)).toBeInTheDocument()
  })

  it('loads behind the page loader, and a session without traces says so', async () => {
    const s = data.byId.get(SHOWCASE_SESSION)!
    server.use(
      http.get('/api/observability/session/:sessionId', () =>
        HttpResponse.json({
          data: { session: { ...sessionDetail(seed, s), traces: [], num_traces: 0 } },
        }),
      ),
    )
    renderApp(`/sessions/${SHOWCASE_SESSION}`)
    expect(await screen.findByTestId('page-loader')).toBeInTheDocument()
    expect(await screen.findByText(copy.noTraces)).toBeInTheDocument()
    expect(screen.getByText(copy.noTracesBody)).toBeInTheDocument()
    expect(screen.queryByTestId('page-loader')).toBeNull()
  })

  it('states: trace store error (500) offers Retry', async () => {
    configureMocks({ variant: 'trace-500' })
    renderApp(`/sessions/${SHOWCASE_SESSION}`)
    expect(await screen.findByText(copy.traceStoreError)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument()
  })

  it('Open in TokenOps jumps to that day with the agent and the spend disclosure open', async () => {
    renderApp(`/sessions/${SHOWCASE_SESSION}?trace=${data.showcaseTraceId}`)
    const link = await screen.findByRole('link', { name: /Open in TokenOps/ }, { timeout: 8000 })
    expect(link.getAttribute('href')).toMatch(new RegExp(`^/tokenops\\?.*day=${seed.spikeDate}`))
    expect(link.getAttribute('href')).toMatch(/agent=seed-code-reviewer/)
    expect(link.getAttribute('href')).toMatch(/open=spend/)
  })

  it('content capture off: the prompt tab explains why it is empty', async () => {
    const user = userEvent.setup()
    const s = data.sessions.find((x) => x.agent.name === 'seed-hr-helpdesk')!
    const llm = generateSpans(seed, s.traces[0].trace_id).find((x) => x.model)!
    renderApp(`/sessions/${s.session_id}?trace=${s.traces[0].trace_id}&span=${llm.hex}`)
    await user.click(
      await screen.findByRole('tab', { name: 'Prompt & response' }, { timeout: 8000 }),
    )
    expect((await screen.findAllByText(copy.captureOff)).length).toBe(2)
  })
})
