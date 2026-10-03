/**
 * Harnesses components in isolation: Breakdown (CSV, empty/error, paging footer, cards),
 * Crumbs collapse, Trend's partial-day rule (QA ISSUE-004), panel states, Individual bits.
 */
import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import type { ReactNode } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import { ApiError } from '@/lib/api/client'
import type { HarnessTotals, UsageRow } from './types'

const csv = vi.hoisted(() => ({ calls: [] as [string, string][] }))
vi.mock('@/features/tokenops/csv', async (orig) => ({
  ...(await orig<typeof import('@/features/tokenops/csv')>()),
  downloadCsv: (name: string, body: string) => void csv.calls.push([name, body]),
}))

const { Breakdown } = await import('./components/Breakdown')
const { Crumbs } = await import('./components/Crumbs')
const { Trend } = await import('./components/Trend')
const { HarnessPanels } = await import('./components/HarnessPanels')
const { ConnectHelp, SessionsList, TopModels } = await import('./components/IndividualView')

const T = (p: Partial<HarnessTotals> = {}): HarnessTotals => ({
  scope_devs: 4,
  active_devs: 2,
  registered_devs: 3,
  idle_seats: 1,
  sessions: 6,
  turns: 10,
  tokens: 1000,
  cost_usd: 12.5,
  unpriced_calls: 0,
  delta_pct: null,
  ...p,
})
const row = (
  label: string,
  kind: UsageRow['kind'],
  breakdown: Record<string, HarnessTotals>,
  totals = T(),
): UsageRow => ({ key: label, label, kind, harness_breakdown: breakdown, totals })
const wrap = (ui: ReactNode) => render(<TooltipProvider>{ui}</TooltipProvider>)

const base = {
  title: 'Units',
  harnesses: ['claude', 'codex'],
  metric: 'active' as const,
  onMetric: () => {},
  overlap: 0,
  onOpen: () => {},
  loading: false,
  stale: false,
  error: null,
  onRetry: () => {},
  team: false,
}

function narrow() {
  vi.stubGlobal('matchMedia', (q: string) => ({
    matches: !q.includes('min-width'),
    media: q,
    addEventListener() {},
    removeEventListener() {},
  }))
}
afterEach(() => {
  vi.unstubAllGlobals()
  csv.calls = []
})

describe('Breakdown', () => {
  it('empty rows say which list is empty; the CSV button is disabled', () => {
    const { unmount } = render(<Breakdown {...base} rows={[]} />)
    expect(screen.getByText('No units to show.')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /CSV/ })).toBeDisabled()
    unmount()
    render(<Breakdown {...base} rows={[]} team />)
    expect(screen.getByText('No developers in this unit.')).toBeInTheDocument()
  })

  it('an error with no rows shows the panel error and retries', async () => {
    const onRetry = vi.fn()
    render(
      <Breakdown
        {...base}
        rows={[]}
        error={new ApiError(500, { error: 'rollup failed', code: 'internal' }, '/api/x', '')}
        onRetry={onRetry}
      />,
    )
    const alert = screen.getByRole('alert')
    expect(alert).toHaveTextContent("Couldn't load the breakdown")
    await userEvent.click(within(alert).getByRole('button', { name: /Retry/ }))
    expect(onRetry).toHaveBeenCalledOnce()
  })

  it('team cells: idle, "—" for not connected, and "unpriced" cost; overlap footnote', async () => {
    const user = userEvent.setup()
    const onMetric = vi.fn()
    const rows = [
      row('Ada', 'user', {
        claude: T({ active_devs: 0, registered_devs: 1 }),
        codex: T({ turns: 10, unpriced_calls: 8 }),
      }),
    ]
    const { rerender } = render(
      <Breakdown {...base} team rows={rows} overlap={1} onMetric={onMetric} />,
    )
    const cells = screen.getAllByRole('cell')
    expect(cells[0]).toHaveTextContent('idle')
    expect(
      screen.getByText(
        '1 developer is counted in more than one unit; the totals count everyone once.',
      ),
    ).toBeInTheDocument()
    await user.click(
      within(screen.getByLabelText('Metric')).getByRole('radio', { name: 'Est. cost' }),
    )
    expect(onMetric).toHaveBeenCalledWith('cost')
    rerender(<Breakdown {...base} team rows={rows} metric="cost" />)
    expect(screen.getAllByRole('cell')[1]).toHaveTextContent('unpriced')
    rerender(<Breakdown {...base} team rows={[row('Bo', 'user', {})]} />)
    expect(screen.getAllByRole('cell')[0]).toHaveTextContent('—')
  })

  it('a partial list says so; the CSV names it in the file name, keeps a parseable header first and blanks unpriced cost', async () => {
    const user = userEvent.setup()
    const rows = [
      row('Ada', 'user', { claude: T({ turns: 10, unpriced_calls: 9, cost_usd: 3 }) }),
      row('Bo', 'user', { claude: T({ cost_usd: 7 }) }),
    ]
    render(<Breakdown {...base} team rows={rows} total={5} hasMore onLoadMore={() => {}} />)
    expect(screen.getByTestId('partial-sort')).toHaveTextContent('Sorted within the 2 loaded of 5')
    expect(screen.getByText('Showing 2 of 5')).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: /CSV/ }))
    const [name, body] = csv.calls[0]!
    expect(name).toBe('harnesses-2-of-5.csv')
    const lines = body.split(/\r?\n/)
    expect(lines[0]).toContain('Claude Code Estimated cost (API list price, USD)')
    const ada = lines.find((l) => l.startsWith('"Ada"'))!
    // Claude cost column is blank for the mostly-unpriced developer; its unpriced count travels with it.
    expect(ada.split(',')[3]).toBe('""')
    expect(ada.split(',')[4]).toBe('"9"')
  })

  it('a failed next page turns Load more into Retry', async () => {
    const onLoadMore = vi.fn()
    render(
      <Breakdown
        {...base}
        team
        rows={[row('Ada', 'user', {})]}
        total={3}
        hasMore
        loadMoreFailed
        onLoadMore={onLoadMore}
      />,
    )
    expect(screen.getByText(/the next page failed to load/)).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }))
    expect(onLoadMore).toHaveBeenCalledOnce()
  })

  it('below the breakpoint it renders one card per row, residual rows last, and a card opens its row', async () => {
    narrow()
    const onOpen = vi.fn()
    const rows = [
      row('Unassigned', 'unassigned', {}, T({ active_devs: 99 })),
      row('Eng', 'unit', { claude: T() }),
    ]
    render(<Breakdown {...base} rows={rows} onOpen={onOpen} />)
    expect(screen.queryByRole('table')).toBeNull()
    const items = within(screen.getByRole('list', { name: 'Units' })).getAllByRole('button')
    expect(items.map((b) => b.textContent)).toEqual([
      expect.stringMatching(/^Eng/),
      expect.stringMatching(/^Unassigned/),
    ])
    await userEvent.click(items[0]!)
    expect(onOpen).toHaveBeenCalledWith(rows[1])
  })
})

describe('Crumbs', () => {
  const items = [
    { label: 'Org', onClick: () => {} },
    { label: 'Eng', onClick: () => {} },
    { label: 'Platform', onClick: () => {} },
    { label: 'Ada' },
  ]
  it('collapses the middle on narrow screens and expands on demand', async () => {
    narrow()
    render(<Crumbs items={items} onClearHarness={() => {}} />)
    const nav = within(screen.getByRole('navigation', { name: 'Breadcrumb' }))
    expect(nav.queryByText('Eng')).toBeNull()
    await userEvent.click(nav.getByRole('button', { name: 'Show full path' }))
    expect(nav.getByRole('button', { name: 'Eng' })).toBeInTheDocument()
    expect(nav.getByText('Ada')).toHaveAttribute('aria-current', 'page')
  })

  it('keeps the full path on wide screens', () => {
    render(<Crumbs items={items} onClearHarness={() => {}} />)
    expect(screen.queryByRole('button', { name: 'Show full path' })).toBeNull()
  })
})

describe('Trend', () => {
  const pt = (date: string, active: number) => ({
    date,
    harness: 'claude',
    active_devs: active,
    cost_usd: active * 0.25,
    tokens: 0,
  })

  // Regression: ISSUE-004 — a 24h window spans two UTC days; hiding today dropped most of its data.
  it('a two-day window keeps today; a longer one hides it ("through yesterday")', () => {
    const { rerender } = render(
      <Trend
        series={[pt('2026-03-20', 1)]}
        harnesses={['claude']}
        days={['2026-03-19', '2026-03-20']}
        today="2026-03-20"
        windowLabel="Last 24 hours"
      />,
    )
    expect(screen.getByText('Daily (UTC)')).toBeInTheDocument()
    expect(screen.getByText(/Claude Code: 1 active/)).toBeInTheDocument()
    rerender(
      <Trend
        series={[pt('2026-03-19', 2), pt('2026-03-20', 1)]}
        harnesses={['claude']}
        days={['2026-03-18', '2026-03-19', '2026-03-20']}
        today="2026-03-20"
        windowLabel="Last 7 days"
      />,
    )
    expect(screen.getByText('Daily (UTC), through yesterday')).toBeInTheDocument()
    // The last value read out is yesterday's, not today's partial day.
    expect(screen.getByText(/Claude Code: 2 active/)).toBeInTheDocument()
  })

  it('switches to Est. cost; an empty series offers a wider window only when there is one', async () => {
    const onWiden = vi.fn()
    const { rerender } = render(
      <Trend
        series={[pt('2026-03-19', 2)]}
        harnesses={['claude', 'codex']}
        days={['2026-03-19']}
        today="2026-03-20"
        windowLabel="Last 7 days"
      />,
    )
    await userEvent.click(
      within(screen.getByLabelText('Trend metric')).getByRole('radio', { name: 'Est. cost' }),
    )
    expect(screen.getByText('Estimated cost per harness')).toBeInTheDocument()
    expect(screen.getByText(/Claude Code: \$0\.50\. Codex: no activity\./)).toBeInTheDocument()
    rerender(
      <Trend
        series={[]}
        harnesses={['claude']}
        days={['2026-03-19']}
        today="2026-03-20"
        windowLabel="Last 7 days"
        onWiden={onWiden}
      />,
    )
    await userEvent.click(screen.getByRole('button', { name: 'Try 30 days' }))
    expect(onWiden).toHaveBeenCalledOnce()
    rerender(
      <Trend
        series={[]}
        harnesses={['claude']}
        days={['2026-03-19']}
        today="2026-03-20"
        windowLabel="Last 30 days"
      />,
    )
    expect(screen.getByText('No harness activity in the last 30 days.')).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Try 30 days' })).toBeNull()
  })
})

describe('HarnessPanels', () => {
  const region = () => within(screen.getByRole('region', { name: 'Harnesses' }))

  it('mostly unpriced reads "unpriced" with "—" ratios; a priced one shows its unpriced count', () => {
    wrap(
      <HarnessPanels
        compare={false}
        onToggle={() => {}}
        items={[
          { harness: 'claude', totals: T({ turns: 10, unpriced_calls: 6 }) },
          { harness: 'codex', totals: T({ turns: 10, unpriced_calls: 2 }) },
        ]}
      />,
    )
    expect(region().getByText('unpriced')).toBeInTheDocument()
    expect(region().getByText('2 unpriced')).toBeInTheDocument()
    expect(region().getAllByText('1 idle')).toHaveLength(2)
  })

  it('compare with no previous window reads "Δ unavailable"; the live form hides sessions and ignores unpriced', () => {
    wrap(
      <HarnessPanels
        compare
        prevUnavailable
        perHarnessUnpricedKnown={false}
        sessionsKnown={false}
        onToggle={() => {}}
        items={[{ harness: 'claude', totals: T({ turns: 10, unpriced_calls: 9 }) }]}
      />,
    )
    expect(region().getByText('Δ unavailable')).toBeInTheDocument()
    expect(region().queryByText('unpriced')).toBeNull()
    expect(region().getByText('$12.50')).toBeInTheDocument()
    expect(region().queryByText('6')).toBeNull()
  })

  it('one or two cards never stretch across the row (QA ISSUE-001: at least 3 columns)', () => {
    wrap(
      <HarnessPanels
        compare={false}
        onToggle={() => {}}
        items={[{ harness: 'claude', totals: T() }]}
      />,
    )
    const style = screen.getByRole('region', { name: 'Harnesses' }).getAttribute('style')!
    expect(style).toContain('--lg-cols: 3')
    expect(style).toContain('--xl-cols: 3')
  })

  it('an unknown harness is labelled Other with its id', () => {
    wrap(
      <HarnessPanels
        compare={false}
        onToggle={() => {}}
        items={[{ harness: 'windsurf', totals: T() }]}
      />,
    )
    expect(region().getByRole('button', { name: 'Filter by Other (windsurf)' })).toBeInTheDocument()
  })
})

describe('Individual pieces', () => {
  it('sessions: empty state, and the degraded row format with unknown counts', () => {
    const { rerender } = render(
      <SessionsList title="Recent sessions" linkable={false} items={[]} />,
    )
    expect(screen.getByText('No harness sessions in this window.')).toBeInTheDocument()
    rerender(
      <SessionsList
        title="Last 20 sessions"
        linkable={false}
        items={[
          {
            session_id: 's',
            harness: 'codex',
            started_at: '2026-03-19T10:00:00Z',
            messages: null,
            tokens: null,
          },
        ]}
      />,
    )
    expect(screen.getByText('— messages · — tokens')).toBeInTheDocument()
    expect(screen.queryByRole('link')).toBeNull()
  })

  it('sessions: only the newest `initial` rows until Show all', async () => {
    const items = Array.from({ length: 12 }, (_, i) => ({
      session_id: `s${i}`,
      harness: 'claude',
      started_at: '2026-03-19T10:00:00Z',
      turns: i,
      cost_usd: 1,
    }))
    render(<SessionsList title="Recent sessions" linkable={false} items={items} initial={8} />)
    expect(screen.getAllByText(/turns ·/)).toHaveLength(8)
    await userEvent.click(screen.getByRole('button', { name: 'Show all 12' }))
    expect(screen.getAllByText(/turns ·/)).toHaveLength(12)
    expect(screen.queryByRole('button', { name: /Show all/ })).toBeNull()
  })

  it('connect help: commands for the viewer, a request line for anyone else', () => {
    const { rerender } = render(
      <ConnectHelp self name="Ada" harness="cursor" server="https://or.example" />,
    )
    expect(screen.getByText(/nasiko connect https:\/\/or\.example/)).toBeInTheDocument()
    expect(screen.getByText(/nasiko agents install cursor/)).toBeInTheDocument()
    rerender(<ConnectHelp self={false} name="Ada" harness="cursor" server="https://or.example" />)
    expect(screen.getByText('Ask Ada to connect Cursor.')).toBeInTheDocument()
  })

  it('Copy puts the three connect commands on the clipboard', async () => {
    const user = userEvent.setup()
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    render(<ConnectHelp self name="Ada" harness="codex" server="https://or.example" />)
    await user.click(screen.getByRole('button', { name: 'Copy' }))
    expect(writeText).toHaveBeenCalledWith(
      'nasiko connect https://or.example\nnasiko auth login\nnasiko agents install codex',
    )
  })

  it('top models render nothing when no harness has any', () => {
    const { container } = render(
      <TopModels byHarness={[{ ...T(), harness: 'claude', top_models: [] }]} />,
    )
    expect(container).toBeEmptyDOMElement()
  })
})
