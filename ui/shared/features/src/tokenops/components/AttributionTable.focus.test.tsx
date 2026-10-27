import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { describe, expect, it, vi } from 'vitest'
import type { AttributionRow } from '../attribution'
import { AttributionTable } from './AttributionTable'

const row = (i: number, kind: AttributionRow['kind'] = 'agent'): AttributionRow => ({
  id: `a${i}`,
  name: `agent ${String(i).padStart(2, '0')}`,
  kind,
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

function Harness({
  linked,
  start = '',
  onPick,
  view = 'agent',
}: {
  linked: boolean
  start?: string
  onPick?: (id: string) => void
  view?: 'agent' | 'workflow'
}) {
  const [focus, setFocus] = useState(start)
  return (
    <AttributionTable
      rows={view === 'agent' ? ROWS : ROWS.map((r) => ({ ...r, kind: 'workflow' as const }))}
      view={view}
      sort="cost"
      loading={false}
      error={null}
      onRetry={() => undefined}
      prevUnavailable
      highlightId={focus || undefined}
      onView={() => undefined}
      onSort={() => undefined}
      onSearch={() => undefined}
      onViewTraces={() => undefined}
      onSelect={
        linked
          ? (id) => {
              onPick?.(id)
              setFocus(id)
            }
          : undefined
      }
      unpriced={false}
      pageSize={10}
    />
  )
}

const show = (props: Parameters<typeof Harness>[0]) =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <Harness {...props} />
    </QueryClientProvider>,
  )

const table = () => screen.getByRole('table')
const focusToggles = () => within(table()).queryAllByRole('button', { name: /^Focus on / })

describe('AttributionTable focus', () => {
  it('offers no focus control when nothing is linked, as on the TokenOps page', () => {
    show({ linked: false })
    expect(focusToggles()).toHaveLength(0)
    expect(within(table()).queryAllByRole('button', { name: /focusing/ })).toHaveLength(0)
    expect(screen.queryByRole('button', { name: /^Focus$/ })).toBeNull()
  })

  it('gives every agent row on the page one focus toggle beside its traces button', () => {
    show({ linked: true })
    expect(focusToggles()).toHaveLength(10)
    expect(within(table()).getAllByRole('button', { name: /^View traces for / })).toHaveLength(10)
  })

  it('focuses a row, marks it pressed and highlighted, and a second press clears it', async () => {
    const onPick = vi.fn()
    show({ linked: true, onPick })
    await userEvent.click(within(table()).getByRole('button', { name: 'Focus on agent 03' }))
    expect(onPick).toHaveBeenLastCalledWith('a3')
    const on = within(table()).getByRole('button', { name: 'Stop focusing on agent 03' })
    expect(on).toHaveAttribute('aria-pressed', 'true')
    expect(on.closest('tr')?.className).toContain('bg-primary/10')
    await userEvent.click(on)
    expect(onPick).toHaveBeenLastCalledWith('')
    expect(within(table()).queryByRole('button', { name: /^Stop focusing/ })).toBeNull()
  })

  it('moves focus from one row to another, leaving exactly one pressed', async () => {
    show({ linked: true })
    await userEvent.click(within(table()).getByRole('button', { name: 'Focus on agent 01' }))
    await userEvent.click(within(table()).getByRole('button', { name: 'Focus on agent 05' }))
    expect(within(table()).getAllByRole('button', { name: /^Stop focusing/ })).toHaveLength(1)
    expect(within(table()).getByRole('button', { name: 'Stop focusing on agent 05' })).toBeVisible()
  })

  it('opens on the page of an agent focused elsewhere', () => {
    show({ linked: true, start: 'a17' })
    expect(within(table()).getByRole('button', { name: 'Stop focusing on agent 17' })).toBeVisible()
  })

  it('has no focus toggle on workflow rows, which name no agent', () => {
    show({ linked: true, view: 'workflow' })
    expect(focusToggles()).toHaveLength(0)
  })

  it('offers the same focus on the phone cards', async () => {
    const onPick = vi.fn()
    show({ linked: true, onPick })
    const cards = screen.getAllByRole('listitem')
    const third = cards.find((li) => within(li).queryByText('agent 03'))
    expect(third).toBeDefined()
    await userEvent.click(within(third as HTMLElement).getByRole('button', { name: 'Focus' }))
    expect(onPick).toHaveBeenLastCalledWith('a3')
    expect(within(third as HTMLElement).getByRole('button', { name: 'In focus' })).toHaveAttribute(
      'aria-pressed',
      'true',
    )
  })
})
