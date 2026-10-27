import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import type { AttributionRow } from '../attribution'
import { toCsv } from '../csv'
import type { SortKey } from '../search'
import { AttributionTable } from './AttributionTable'

vi.mock('../csv', async (load) => ({
  ...(await load<typeof import('../csv')>()),
  downloadCsv: vi.fn(),
  toCsv: vi.fn(() => ''),
}))

const row = (i: number): AttributionRow => ({
  id: `a${i}`,
  name: `agent ${String(i).padStart(2, '0')}`,
  kind: 'agent',
  cost: 100 - i,
  sharePct: 1,
  deltaPct: null,
  deltaKind: 'unavailable',
  tokens: 1000 + i,
  operations: 10,
  costPerOp: 0.1,
  latency: 100,
  p95: 200,
  cacheRatioPct: null,
  containerHours: null,
  capped: false,
})

const ROWS = Array.from({ length: 23 }, (_, i) => row(i + 1))

function Harness({ pageSize, highlightId }: { pageSize?: number; highlightId?: string }) {
  const [sort, setSort] = useState<SortKey>('cost')
  const [q, setQ] = useState('')
  const [view, setView] = useState<'agent' | 'workflow'>('agent')
  return (
    <AttributionTable
      rows={ROWS}
      view={view}
      sort={sort}
      q={q}
      loading={false}
      error={null}
      onRetry={() => undefined}
      prevUnavailable
      highlightId={highlightId}
      onView={setView}
      onSort={setSort}
      onSearch={setQ}
      onViewTraces={() => undefined}
      unpriced={false}
      pageSize={pageSize}
    />
  )
}

const show = (props: { pageSize?: number; highlightId?: string } = {}) =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Harness {...props} />
    </QueryClientProvider>,
  )

const names = () =>
  within(screen.getByRole('table'))
    .getAllByRole('row')
    .slice(1)
    .map((r) => within(r).getAllByRole('cell')[0]?.textContent ?? '')

const pager = () => screen.queryByRole('navigation', { name: /pagination/i })

describe('AttributionTable paging', () => {
  it('shows the top rows and Show all, never a pager, without a page size (the TokenOps page)', async () => {
    show()
    expect(names()).toHaveLength(10)
    expect(pager()).not.toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Show all 23' }))
    expect(names()).toHaveLength(23)
    expect(pager()).not.toBeInTheDocument()
  })

  it('pages rows by the page size', async () => {
    show({ pageSize: 10 })
    expect(names()).toHaveLength(10)
    expect(names()[0]).toBe('agent 01')
    await userEvent.click(screen.getByRole('link', { name: '3' }))
    expect(names()).toEqual(['agent 21', 'agent 22', 'agent 23'])
    await userEvent.click(screen.getByRole('link', { name: /previous/i }))
    expect(names()[0]).toBe('agent 11')
  })

  it('returns to the first page when the search changes', async () => {
    show({ pageSize: 10 })
    await userEvent.click(screen.getByRole('link', { name: '2' }))
    expect(names()[0]).toBe('agent 11')
    await userEvent.type(screen.getByRole('searchbox', { name: /search agents/i }), 'agent')
    expect(names()[0]).toBe('agent 01')
    expect(screen.getByRole('link', { name: '1' })).toHaveAttribute('aria-current', 'page')
  })

  it('returns to the first page when the sort changes', async () => {
    show({ pageSize: 10 })
    await userEvent.click(screen.getByRole('link', { name: '3' }))
    await userEvent.click(screen.getByRole('combobox', { name: 'Sort by' }))
    await userEvent.click(await screen.findByRole('option', { name: 'Most tokens' }))
    expect(names()[0]).toBe('agent 23')
    expect(screen.getByRole('link', { name: '1' })).toHaveAttribute('aria-current', 'page')
  })

  it('opens the page that holds the highlighted row', () => {
    show({ pageSize: 10, highlightId: 'a22' })
    expect(names()).toContain('agent 22')
    expect(screen.getByRole('link', { name: '3' })).toHaveAttribute('aria-current', 'page')
  })

  it('pages the narrow-screen cards the same way', () => {
    show({ pageSize: 10 })
    const cards = screen
      .getAllByRole('list')
      .find((list) => within(list).queryByText('agent 01') !== null)
    expect(cards && within(cards).getAllByRole('listitem')).toHaveLength(10)
  })

  it('exports every matching row to CSV, not one page', async () => {
    show({ pageSize: 10 })
    await userEvent.click(screen.getByRole('button', { name: /csv/i }))
    expect(vi.mocked(toCsv).mock.lastCall?.[1]).toHaveLength(23)
  })
})
