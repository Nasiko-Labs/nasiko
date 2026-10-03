/**
 * The 1 s tick (plan §8 Phase 8, TODOS "Streaming render cost"): only the line that shows a live turn's
 * elapsed time ticks; the page's turns don't re-render on it.
 */
import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, expect, it, vi } from 'vitest'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { clearDrafts } from './drafts'
import { clearChatRegistry } from './registry'
import { tuning } from './tuning'

const DONE = '5eedc000-0000-4000-8000-00000000c001'

/** Each time the page renders a turn (the wrapper renders only when the page hands it new props). */
const renders = vi.hoisted(() => ({ n: 0 }))
vi.mock('./components/TurnView', async (importOriginal) => {
  const m = await importOriginal<typeof import('./components/TurnView')>()
  const { memo } = await import('react')
  type Props = Parameters<typeof m.TurnView>[0]
  // The memo's own component, so the count is the turn's real renders.
  const Body = (m.TurnView as unknown as { type: (props: Props) => React.ReactNode }).type
  return {
    ...m,
    TurnView: memo(function CountedTurnView(props: Props) {
      renders.n++
      return <Body {...props} />
    }),
  }
})

afterEach(() => {
  vi.useRealTimers()
  clearChatRegistry()
  clearDrafts()
})

it('a waiting turn counts its seconds without re-rendering the turns', async () => {
  server.use(
    http.post(
      '/api/orchestrator/a2a',
      () =>
        new HttpResponse(new ReadableStream({ start() {} }), {
          headers: { 'Content-Type': 'text/event-stream' },
        }),
    ),
  )
  vi.useFakeTimers({ shouldAdvanceTime: true })
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
  renderApp(`/chat/${DONE}`)
  await user.type(await screen.findByLabelText(/^Message /), 'take your time{Enter}')
  expect(await screen.findByText(/^Waiting for /)).toBeInTheDocument()
  await vi.advanceTimersByTimeAsync(tuning.SLOW_MS + 1_000)
  const before = (await screen.findByText(/^\d+s$/)).textContent
  const rendered = renders.n
  await vi.advanceTimersByTimeAsync(3_000)
  expect(screen.getByText(/^\d+s$/).textContent).not.toBe(before)
  expect(renders.n).toBe(rendered)
})
