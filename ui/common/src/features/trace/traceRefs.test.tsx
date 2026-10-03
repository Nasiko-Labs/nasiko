/**
 * Trace page patterns borrowed from Axiom, Braintrust and Extend (feat/trace-refs):
 * the rollup line, "Filter spans", previous/next span (following the Table view, the filter and collapsed
 * parents; the panel tab stays put), and the Input/Output JSON view.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from 'vitest'
import { copy } from '@/features/observability/copy'
import { generateSpans, observabilityData, SHOWCASE_SESSION } from '@/mocks/observability'
import { seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'

setupPinnedSeed()

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
const showcase = `/sessions/${SHOWCASE_SESSION}?trace=${data.showcaseTraceId}`
const panelTitle = () =>
  screen.getByRole('heading', { level: 2, name: (_, el) => el?.id === 'span-panel-title' })
    .textContent

describe('trace page reference patterns', () => {
  it('shows a rollup of span kinds and duration under the header', async () => {
    renderApp(showcase)
    expect(
      await screen.findByText(
        /19 spans · \d+ LLM calls · \d+ tool calls · 3 agent calls · [\d.]+ s/,
        {},
        { timeout: 8000 },
      ),
    ).toBeInTheDocument()
  })

  it('Filter spans narrows the count and the table', async () => {
    const user = userEvent.setup()
    renderApp(showcase)
    await screen.findByRole('tree', { name: 'Spans' }, { timeout: 8000 })
    await user.type(screen.getByRole('searchbox', { name: 'Filter spans' }), 'repair')
    expect(await screen.findByText('5 of 19 spans')).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Table' }))
    const rows = within(screen.getByRole('table')).getAllByRole('row').slice(1)
    expect(rows).toHaveLength(5)
    expect(rows.every((r) => r.textContent?.includes('llm.repair_args'))).toBe(true)
  })

  it('previous/next span walk the tree and keep ?trace and ?span in the URL', async () => {
    const user = userEvent.setup()
    const plan = generateSpans(seed, data.showcaseTraceId).find((s) => s.name === 'llm.plan')!
    const { router } = renderApp(`${showcase}&span=${plan.hex}`)
    expect(await screen.findByText(/^· 2 of 19$/, {}, { timeout: 8000 })).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Next span' }))
    await waitFor(() =>
      expect(screen.getByRole('heading', { level: 2, name: 'tool.get_diff' })).toBeInTheDocument(),
    )
    expect(router.state.location.search).toMatchObject({ trace: data.showcaseTraceId })
    await user.click(screen.getByRole('button', { name: 'Previous span' }))
    await waitFor(() =>
      expect(screen.getByRole('heading', { level: 2, name: 'llm.plan' })).toBeInTheDocument(),
    )
  })

  it('Input/Output offer a JSON view when the content parses', async () => {
    const user = userEvent.setup()
    const repair = generateSpans(seed, data.showcaseTraceId).find(
      (s) => s.name === 'llm.repair_args',
    )!
    renderApp(`${showcase}&span=${repair.hex}`)
    await user.click(
      await screen.findByRole('tab', { name: 'Prompt & response' }, { timeout: 8000 }),
    )
    const view = await screen.findByRole('group', { name: 'Output view' })
    await user.click(within(view).getByRole('button', { name: 'JSON' }))
    expect(
      within(screen.getByRole('tabpanel')).getByText(/"ref": "refs\/pull\/481\/head"/),
    ).toBeInTheDocument()
  })

  // Regressions from /review of feat/trace-refs (2026-09-26).
  it('Filter spans marks non-matching tree rows as filtered out (dimming is never the only signal)', async () => {
    const user = userEvent.setup()
    renderApp(showcase)
    const tree = await screen.findByRole('tree', { name: 'Spans' }, { timeout: 8000 })
    await user.type(screen.getByRole('searchbox', { name: 'Filter spans' }), 'repair')
    await screen.findByText('5 of 19 spans')
    expect(within(tree).getByRole('treeitem', { name: /^llm\.plan,.*filtered out$/ })).toHaveClass(
      'opacity-50',
    )
    const repairs = within(tree).getAllByRole('treeitem', { name: /^llm\.repair_args/ })
    expect(repairs.length).toBeGreaterThan(0)
    for (const r of repairs) expect(r).not.toHaveAccessibleName(/filtered out$/)
  })

  it('Filter spans: no match shows 0 of N and a no-match row; model ids match, case-insensitively', async () => {
    const user = userEvent.setup()
    const model = generateSpans(seed, data.showcaseTraceId).find((s) => s.model)!.model!
    const llmCount = generateSpans(seed, data.showcaseTraceId).filter((s) => s.model).length
    renderApp(showcase)
    await screen.findByRole('tree', { name: 'Spans' }, { timeout: 8000 })
    const box = screen.getByRole('searchbox', { name: 'Filter spans' })
    await user.type(box, 'zzz-nothing')
    expect(await screen.findByText('0 of 19 spans')).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Table' }))
    expect(screen.getByText(copy.noSpansMatch)).toBeInTheDocument()
    await user.clear(box)
    await user.type(box, `  ${model.toUpperCase()} `)
    expect(await screen.findByText(`${llmCount} of 19 spans`)).toBeInTheDocument()
  })

  it('Previous is inert on the first span and Next on the last; focus stays on the button', async () => {
    const user = userEvent.setup()
    const root = generateSpans(seed, data.showcaseTraceId)[0]
    renderApp(`${showcase}&span=${root.hex}`)
    const prev = await screen.findByRole('button', { name: 'Previous span' }, { timeout: 8000 })
    expect(screen.getByText(/^· 1 of 19$/)).toBeInTheDocument()
    expect(prev).toHaveAttribute('aria-disabled', 'true')
    const next = screen.getByRole('button', { name: 'Next span' })
    await user.click(next)
    // The panel stays mounted while stepping, so focus stays on Next.
    expect(await screen.findByText(/^· 2 of 19$/)).toBeInTheDocument()
    expect(next).toHaveFocus()
    for (let i = 2; i < 19; i++) await user.click(next)
    expect(await screen.findByText(/^· 19 of 19$/)).toBeInTheDocument()
    expect(next).toHaveAttribute('aria-disabled', 'true')
    await user.click(next)
    expect(screen.getByText(/^· 19 of 19$/)).toBeInTheDocument()
    expect(next).toHaveFocus()
  })

  describe('Copy span ID', () => {
    afterEach(() => vi.unstubAllGlobals())
    it('writes the hex id and says copied; a rejected clipboard says nothing', async () => {
      const user = userEvent.setup()
      const plan = generateSpans(seed, data.showcaseTraceId).find((s) => s.name === 'llm.plan')!
      const writeText = vi.fn().mockResolvedValue(undefined)
      Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
      renderApp(`${showcase}&span=${plan.hex}`)
      await user.click(
        await screen.findByRole('button', { name: 'Copy span ID' }, { timeout: 8000 }),
      )
      expect(writeText).toHaveBeenCalledWith(plan.hex)
      expect(await screen.findByText(copy.copied)).toBeInTheDocument()
      writeText.mockRejectedValueOnce(new Error('denied'))
      await waitFor(() => expect(screen.queryByText(copy.copied)).toBeNull(), { timeout: 8000 })
      await user.click(screen.getByRole('button', { name: 'Copy span ID' }))
      await new Promise((r) => setTimeout(r, 50))
      expect(screen.queryByText(copy.copied)).toBeNull()
    })
  })

  it('attribute groups collapse; the JSON toggle is absent for plain-text output', async () => {
    const user = userEvent.setup()
    const plan = generateSpans(seed, data.showcaseTraceId).find((s) => s.name === 'llm.plan')!
    renderApp(`${showcase}&span=${plan.hex}`)
    await user.click(
      await screen.findByRole('tab', { name: 'Prompt & response' }, { timeout: 8000 }),
    )
    await screen.findByRole('button', { name: 'Output' })
    expect(screen.queryByRole('group', { name: 'Output view' })).toBeNull()
    await user.click(screen.getByRole('tab', { name: 'Attributes' }))
    const group = await screen.findByRole('button', { name: /^gen_ai \d+$/ })
    expect(group).toHaveAttribute('aria-expanded', 'true')
    expect(screen.getByTitle('gen_ai.request.model')).toBeInTheDocument()
    await user.click(group)
    expect(group).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByTitle('gen_ai.request.model')).toBeNull()
  })
})

describe('previous/next follow what is on screen', () => {
  it('in the Table view, Next selects the next table row (start order)', async () => {
    const user = userEvent.setup()
    renderApp(showcase)
    await screen.findByRole('tree', { name: 'Spans' }, { timeout: 8000 })
    await user.click(screen.getByRole('button', { name: 'Table' }))
    const names = () =>
      within(screen.getByRole('table'))
        .getAllByRole('row')
        .slice(1)
        .map((r) => within(r).getByRole('button'))
    await user.click(names()[2])
    await waitFor(() => expect(panelTitle()).toBe(names()[2].textContent))
    await user.click(screen.getByRole('button', { name: 'Next span' }))
    await waitFor(() => expect(panelTitle()).toBe(names()[3].textContent))
    expect(screen.getByText(/^· 4 of 19$/)).toBeInTheDocument()
  })

  it('with a filter on, stepping walks only the matches', async () => {
    const user = userEvent.setup()
    const repairs = generateSpans(seed, data.showcaseTraceId).filter(
      (s) => s.name === 'llm.repair_args',
    )
    renderApp(`${showcase}&span=${repairs[0].hex}`)
    await screen.findByRole('tree', { name: 'Spans' }, { timeout: 8000 })
    await user.type(screen.getByRole('searchbox', { name: 'Filter spans' }), 'repair')
    expect(await screen.findByText(/^· 1 of 5$/)).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Next span' }))
    expect(await screen.findByText(/^· 2 of 5$/)).toBeInTheDocument()
    expect(panelTitle()).toBe('llm.repair_args')
  })

  it(
    'stepping into a collapsed node opens it so the selection stays visible',
    { timeout: 15_000 },
    async () => {
      const user = userEvent.setup()
      // A normal (non-storm) trace whose retriever.search has children (embeddings + vector search).
      const found = data.sessions
        .filter((x) => x.agent.name === 'seed-invoice-parser')
        .flatMap((x) => x.traces.map((t) => ({ session: x, t })))
        .map(({ session, t }) => ({
          session,
          t,
          retrieval: generateSpans(seed, t.trace_id).find((sp) => sp.name === 'retriever.search'),
        }))
        .find((x) => x.retrieval)!
      renderApp(
        `/sessions/${found.session.session_id}?trace=${found.t.trace_id}&span=${found.retrieval!.hex}`,
      )
      const tree = await screen.findByRole('tree', { name: 'Spans' }, { timeout: 8000 })
      const row = await within(tree).findByRole('treeitem', { selected: true })
      expect(row).toHaveAccessibleName(/^retriever\.search/)
      row.focus()
      await user.keyboard('{ArrowLeft}')
      await waitFor(() =>
        expect(within(tree).getByRole('treeitem', { selected: true })).toHaveAttribute(
          'aria-expanded',
          'false',
        ),
      )
      expect(within(tree).queryByRole('treeitem', { name: /^embeddings\.create/ })).toBeNull()
      await user.click(screen.getByRole('button', { name: 'Next span' }))
      await waitFor(() => expect(panelTitle()).toBe('embeddings.create'))
      expect(within(tree).getByRole('treeitem', { selected: true })).toHaveAccessibleName(
        /^embeddings\.create/,
      )
      expect(within(tree).getByRole('treeitem', { name: /^retriever\.search/ })).toHaveAttribute(
        'aria-expanded',
        'true',
      )
    },
  )

  it('the selected panel tab stays put while stepping', async () => {
    const user = userEvent.setup()
    const plan = generateSpans(seed, data.showcaseTraceId).find((s) => s.name === 'llm.plan')!
    renderApp(`${showcase}&span=${plan.hex}`)
    await user.click(
      await screen.findByRole('tab', { name: 'Prompt & response' }, { timeout: 8000 }),
    )
    await user.click(screen.getByRole('button', { name: 'Next span' }))
    await waitFor(() => expect(panelTitle()).not.toBe('llm.plan'))
    expect(screen.getByRole('tab', { name: 'Prompt & response' })).toHaveAttribute(
      'aria-selected',
      'true',
    )
  })

  it('switching trace clears the span filter', async () => {
    const user = userEvent.setup()
    renderApp(showcase)
    await screen.findByRole('tree', { name: 'Spans' }, { timeout: 8000 })
    const box = screen.getByRole('searchbox', { name: 'Filter spans' })
    await user.type(box, 'repair')
    await screen.findByText('5 of 19 spans')
    await user.click(screen.getByRole('combobox', { name: /^Trace \(\d+ in this session\)$/ }))
    const options = await screen.findAllByRole('option')
    await user.click(options.find((o) => !o.textContent?.includes('failing'))!)
    await waitFor(() =>
      expect(screen.getByRole('searchbox', { name: 'Filter spans' })).toHaveValue(''),
    )
  })
})
