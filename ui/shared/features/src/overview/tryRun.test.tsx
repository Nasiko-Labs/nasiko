import { act, fireEvent, render, screen } from '@testing-library/react'
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { TryAgentRun } from './components/TryAgentRun'
import { copy } from './copy'
import { nextPhase, readDone, showTryRun, TRY_RUN_AGENT, writeDone } from './tryRun'
import { TRY_RUN_COMPLETED_MS, TRY_RUN_RUNNING_MS, TRY_RUN_STARTING_MS } from './tuning'

const t = copy.tryRun

describe('try-a-run rules', () => {
  it('walks starting → running → completed → idle', () => {
    expect(nextPhase('starting')).toEqual({ next: 'running', afterMs: TRY_RUN_STARTING_MS })
    expect(nextPhase('running')).toEqual({ next: 'completed', afterMs: TRY_RUN_RUNNING_MS })
    expect(nextPhase('completed')).toEqual({ next: 'idle', afterMs: TRY_RUN_COMPLETED_MS })
    expect(nextPhase('idle')).toBeNull()
  })

  it('shows with no activity and between the runs, never in a workspace with spend', () => {
    expect(showTryRun([], 0)).toBe(true)
    expect(showTryRun([], 12)).toBe(false)
    expect(showTryRun([], null)).toBe(false)
    expect(showTryRun(['baseline'], 1)).toBe(true)
    expect(showTryRun(['baseline', 'optimized'], 12)).toBe(false)
    expect(showTryRun(['baseline', 'optimized'], null)).toBe(false)
    expect(showTryRun(['baseline', 'optimized'], 0)).toBe(true)
  })

  it('keeps finished runs per viewer', () => {
    expect(readDone('u1')).toEqual([])
    writeDone('u1', 'baseline')
    expect(readDone('u1')).toEqual(['baseline'])
    writeDone('u1', 'optimized')
    expect(readDone('u1')).toEqual(['baseline', 'optimized'])
    expect(readDone('u2')).toEqual([])
  })
})

function renderCard(userId = 'u1') {
  const rootRoute = createRootRoute({ component: () => <TryAgentRun userId={userId} /> })
  const router = createRouter({
    routeTree: rootRoute,
    history: createMemoryHistory({ initialEntries: ['/'] }),
  })
  return render(<RouterProvider router={router} />)
}

describe('TryAgentRun', () => {
  beforeEach(() => vi.useFakeTimers({ shouldAdvanceTime: true }))
  afterEach(() => vi.useRealTimers())

  const advance = (ms: number) => act(() => void vi.advanceTimersByTime(ms))

  it('plays the run on the button, then swaps it for the optimized run', async () => {
    renderCard()
    const button = await screen.findByRole('button', { name: t.baseline })
    expect(screen.queryByRole('button', { name: t.optimized })).toBeNull()

    fireEvent.click(button)
    expect(screen.getByRole('button', { name: t.starting })).toBeDisabled()
    advance(TRY_RUN_STARTING_MS)
    expect(screen.getByRole('button', { name: t.running })).toBeDisabled()
    advance(TRY_RUN_RUNNING_MS)
    expect(screen.getByRole('button', { name: t.completed })).toBeInTheDocument()
    expect(screen.getByText(t.baselineDone(TRY_RUN_AGENT))).toBeInTheDocument()
    expect(screen.getByRole('link', { name: t.seeRun })).toHaveAttribute('href', '/sessions')
    expect(screen.getByText(t.refreshStats)).toBeInTheDocument()
    advance(TRY_RUN_COMPLETED_MS)
    expect(screen.queryByRole('button', { name: t.baseline })).toBeNull()
    expect(screen.getByText(t.nextStep)).toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: t.optimized }))
    advance(TRY_RUN_STARTING_MS)
    advance(TRY_RUN_RUNNING_MS)
    expect(screen.getByText(t.optimizedDone(TRY_RUN_AGENT))).toBeInTheDocument()
    expect(screen.getByText(t.refreshOptimized)).toBeInTheDocument()
    expect(readDone('u1')).toEqual(['baseline', 'optimized'])
  })

  it('offers the optimized run after a refresh once the first run is done', async () => {
    writeDone('u1', 'baseline')
    renderCard()
    expect(await screen.findByRole('button', { name: t.optimized })).toBeEnabled()
    expect(screen.queryByRole('button', { name: t.baseline })).toBeNull()
    expect(screen.queryByText(t.baselineDone(TRY_RUN_AGENT))).toBeNull()
    expect(screen.getByText(t.nextStep)).toBeInTheDocument()
  })
})
