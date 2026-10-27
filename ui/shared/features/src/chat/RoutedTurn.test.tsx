/**
 * Routed turn pieces in isolation (v1b §5.5-§5.7): the branches of RoutedLive and Activity the
 * page tests don't reach — notice actions per error key, a resume's end states, the live status
 * line's timers, the idle notice, expanded rows while live, and the done footer's trace button.
 */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import {
  createMemoryHistory,
  createRootRoute,
  createRouter,
  RouterProvider,
} from '@tanstack/react-router'
import { render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import type { ReactNode } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { emptyTurn, type Step, type TurnState } from './a2aReducer'
import { Activity, RoutedLive, type RoutedHandlers } from './components/RoutedTurn'
import { ChatError } from './errors'
import { tuning } from './tuning'
import type { LiveTurn, Phase } from './turnRegistry'

const TRACE = '5eedf000000000000000000000000c0v'

async function inRouter(ui: ReactNode) {
  const router = createRouter({
    routeTree: createRootRoute({ component: () => <>{ui}</> }),
    history: createMemoryHistory({ initialEntries: ['/'] }),
  })
  render(
    <QueryClientProvider client={new QueryClient()}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  )
  // The root route renders asynchronously.
  await screen.findAllByTestId(/routed-live|activity|known-empty/)
}

function live(
  over: Omit<Partial<LiveTurn>, 'state'> & { state?: Partial<TurnState> } = {},
): LiveTurn {
  const { state, ...rest } = over
  return {
    id: 't1',
    sessionId: 's1',
    agentId: null,
    userText: 'hello there',
    userMessageId: 'u1',
    phase: 'streaming' as Phase,
    state: { ...emptyTurn(), ...state },
    startedAt: 0,
    idle: false,
    stopped: false,
    finalized: false,
    error: null,
    saved: null,
    pendingSave: null,
    frames: [],
    chatMode: 'routed',
    operation: 'send',
    attempt: 'send',
    ...rest,
  }
}

function handlers(): RoutedHandlers & { [k: string]: ReturnType<typeof vi.fn> } {
  return { viewTrace: vi.fn(), refresh: vi.fn(), runAgain: vi.fn(), editMessage: vi.fn() }
}

const ctx = { agents: [], endFor: () => undefined }

const step = (over: Partial<Step>): Step => ({
  key: `${over.name}#1`,
  kind: 'agent',
  name: 'alpha',
  status: 'ok',
  ...over,
})

describe('RoutedLive notice actions (§5.6)', () => {
  it('a 400 offers Edit and send, which puts the user text back', async () => {
    const h = handlers()
    await inRouter(
      <RoutedLive
        live={live({
          phase: 'not_started',
          finalized: true,
          error: new ChatError({
            phase: 'dispatch',
            key: 'routedBadRequest',
            certainty: 'rejected-before-run',
            status: 400,
          }),
        })}
        sessionId="s1"
        hasSaved={false}
        now={0}
        ctx={ctx}
        handlers={h}
        tryAgain={vi.fn()}
      />,
    )
    await userEvent.click(screen.getByRole('button', { name: 'Edit and send' }))
    expect(h.editMessage).toHaveBeenCalledWith('hello there')
    expect(screen.queryByRole('button', { name: 'Run again' })).toBeNull()
  })

  it('an error with no ChatError still shows the cut-off notice with its trace link', async () => {
    const h = handlers()
    await inRouter(
      <RoutedLive
        live={live({ phase: 'error', finalized: true, state: { traceId: TRACE } })}
        sessionId="s1"
        hasSaved={false}
        now={0}
        ctx={ctx}
        handlers={h}
        tryAgain={vi.fn()}
      />,
    )
    await userEvent.click(
      within(screen.getByTestId('error-notice')).getByRole('button', { name: 'View trace' }),
    )
    expect(h.viewTrace).toHaveBeenCalledWith(TRACE, true)
  })
})

describe('RoutedLive resume end states (§5.4: a resume never re-runs)', () => {
  it('lost with partial text on a resume offers Refresh status and no Run again', async () => {
    const state: Partial<TurnState> = {
      artifacts: [{ id: 'a', text: 'half of it' }] as TurnState['artifacts'],
      replyArtifactsFrom: 0,
      traceId: TRACE,
    }
    await inRouter(
      <RoutedLive
        live={live({ phase: 'lost', operation: 'resume', finalized: true, state })}
        sessionId="s1"
        hasSaved={false}
        now={0}
        ctx={ctx}
        handlers={handlers()}
        tryAgain={vi.fn()}
      />,
    )
    expect(screen.getByText('Partial reply — completion unconfirmed')).toBeInTheDocument()
    expect(screen.getByText('half of it')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Refresh status' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Run again' })).toBeNull()
  })

  it('an empty resume ending says finished without a reply, with Refresh status instead of Run again', async () => {
    const h = handlers()
    await inRouter(
      <RoutedLive
        live={live({ phase: 'known_empty', operation: 'resume', finalized: true })}
        sessionId="s1"
        hasSaved={false}
        now={0}
        ctx={ctx}
        handlers={h}
        tryAgain={vi.fn()}
      />,
    )
    await userEvent.click(
      within(screen.getByTestId('known-empty')).getByRole('button', { name: 'Refresh status' }),
    )
    expect(h.refresh).toHaveBeenCalledOnce()
    expect(h.runAgain).not.toHaveBeenCalled()
  })
})

describe('RoutedLive while live (§5.5)', () => {
  it('the status line shows the elapsed time past SLOW_MS and the still-working note past LONG_MS', async () => {
    await inRouter(
      <RoutedLive
        live={live({ phase: 'streaming', startedAt: 0 })}
        sessionId="s1"
        hasSaved={false}
        now={tuning.LONG_MS + 1000}
        ctx={ctx}
        handlers={handlers()}
        tryAgain={vi.fn()}
      />,
    )
    const line = screen.getByTestId('routed-status')
    expect(line).toHaveTextContent('Working')
    expect(line).toHaveTextContent(`${Math.floor((tuning.LONG_MS + 1000) / 1000)}s`)
    expect(line).toHaveTextContent(/Still working/)
  })

  it('an idle stream says so, with Refresh status', async () => {
    const h = handlers()
    await inRouter(
      <RoutedLive
        live={live({ phase: 'streaming', idle: true })}
        sessionId="s1"
        hasSaved={false}
        now={0}
        ctx={ctx}
        handlers={h}
        tryAgain={vi.fn()}
      />,
    )
    expect(screen.getByText(/No update for a while/)).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Refresh status' }))
    expect(h.refresh).toHaveBeenCalledOnce()
  })

  it('a finished reply (history not caught up) shows attribution and a footer whose View trace is fresh', async () => {
    const h = handlers()
    const state: Partial<TurnState> = {
      artifacts: [{ id: 'a', text: 'All good.' }] as TurnState['artifacts'],
      replyArtifactsFrom: 0,
      traceId: TRACE,
      taskState: 'completed',
      steps: [step({ name: 'alpha', status: 'ok' })],
    }
    await inRouter(
      <RoutedLive
        live={live({ phase: 'done', finalized: true, state })}
        sessionId="s1"
        hasSaved={false}
        now={0}
        ctx={ctx}
        handlers={h}
        tryAgain={vi.fn()}
      />,
    )
    expect(screen.getByTestId('attribution')).toHaveTextContent('alpha')
    await userEvent.click(screen.getByRole('button', { name: 'View trace' }))
    expect(h.viewTrace).toHaveBeenCalledWith(TRACE, true)
  })
})

describe('Activity rows (§5.7)', () => {
  it('live: a running agent shows its sub_status under the row; expanded calls show running/failed with times and a trace link', async () => {
    const onViewTrace = vi.fn()
    const state = {
      steps: [
        step({ key: 'alpha#1', name: 'alpha', status: 'running' }),
        step({ key: 'beta#1', name: 'beta', status: 'error', durationMs: 1200, detail: 'boom' }),
      ],
      agentNotes: { alpha: { status: 'reading the logs' } },
    }
    await inRouter(
      <Activity state={state} live agents={[]} traceId={TRACE} onViewTrace={onViewTrace} />,
    )
    await userEvent.click(screen.getByRole('button', { name: /Activity · 2 agents/ }))
    expect(screen.getByRole('button', { name: /Activity · 2 agents/ })).toHaveTextContent(
      '1 failed',
    )
    expect(screen.getByText('reading the logs')).toBeInTheDocument()
    const alpha = screen.getByRole('button', { name: /alpha/ })
    expect(alpha).toHaveTextContent('Running')
    const beta = screen.getByRole('button', { name: /beta/ })
    expect(beta).toHaveTextContent('Failed')
    expect(beta).toHaveTextContent('Total call time')
    await userEvent.click(alpha)
    await userEvent.click(beta)
    expect(screen.getByText('running')).toBeInTheDocument()
    expect(screen.getByText('failed')).toBeInTheDocument()
    // The failed call's result is its excerpt.
    expect(screen.getAllByTestId('activity-excerpt').map((e) => e.textContent)).toEqual(['boom'])
    await userEvent.click(screen.getAllByRole('button', { name: 'View trace' })[0]!)
    expect(onViewTrace).toHaveBeenCalledWith(TRACE)
  })
})
