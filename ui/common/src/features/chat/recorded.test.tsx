/**
 * Chat v1c M1, recorded harness chats (plans/feat-chat-v1c.md §5.7, §7 tests 4 and 12): the tool-call
 * mapping, the chips (collapse, detail, Copy full value, scale), the metadata-only state, the rail's
 * Chats / Recorded views and axe on the recorded screens.
 */
import { render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import axe from 'axe-core'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { configureChatMock } from '@/mocks/chatStore'
import { ADMIN_ID } from '@/mocks/seed-harness'
import { renderApp } from '@/test/renderApp'
import { RecordedCalls } from './components/RecordedCalls'
import { copy } from './copy'
import { clearDrafts } from './drafts'
import {
  isEmptyValue,
  isMetadataOnly,
  RECORDED_STATUSES,
  sectionText,
  toolCallsToSteps,
} from './recorded'
import { clearChatRegistry } from './registry'
import { readRailView, rememberRailView } from './rememberTarget'
import { tuning } from './tuning'

const RECORDED = '5eedc000-0000-4000-8000-00000000c004'
const METADATA_ONLY = '5eedc000-0000-4000-8000-00000000c005'

const meta = (calls: unknown, policy = 'content') => ({
  coding_agent: { capture_policy: policy, tool_calls: calls },
})
const call = (over: Record<string, unknown> = {}) => ({
  id: 't1',
  name: 'bash',
  kind: 'shell',
  status: 'succeeded',
  association: 'exact',
  timestamp_quality: 'exact',
  ...over,
})

afterEach(() => {
  clearChatRegistry()
  clearDrafts()
  vi.unstubAllGlobals()
})

describe('toolCallsToSteps (test 4)', () => {
  it.each([
    ['succeeded', 'ok', undefined],
    ['failed', 'error', 'failed'],
    ['denied', 'error', 'denied'],
    ['timed_out', 'error', 'timed out'],
    ['cancelled', 'error', 'cancelled'],
    ['pending', 'neutral', 'No result recorded'],
    ['running', 'neutral', 'No result recorded'],
    ['unknown', 'neutral', 'Status unknown'],
  ] as const)('%s → %s', (status, kind, word) => {
    const [step] = toolCallsToSteps(meta([call({ status })]))!.steps
    expect(step).toMatchObject({ recordedStatus: status, status: kind, statusWord: word })
  })

  it('covers the whole server enum', () => {
    expect([...RECORDED_STATUSES].sort()).toEqual([
      'cancelled',
      'denied',
      'failed',
      'pending',
      'running',
      'succeeded',
      'timed_out',
      'unknown',
    ])
  })

  it('takes the duration from duration_ms, else exact timestamps only; relative time only when exact', () => {
    const at = '2026-03-20T10:00:00.000Z'
    const later = '2026-03-20T10:00:02.500Z'
    const steps = toolCallsToSteps(
      meta([
        call({ duration_ms: 40, started_at: at, ended_at: later }),
        call({ started_at: at, ended_at: later }),
        call({ started_at: at, ended_at: later, timestamp_quality: 'inferred' }),
      ]),
    )!.steps
    expect(steps.map((s) => s.durationMs)).toEqual([40, 2500, undefined])
    expect(steps.map((s) => s.startedAt)).toEqual([at, at, undefined])
  })

  it('keeps only the sections the call has, raw; tags turn and unknown associations', () => {
    const steps = toolCallsToSteps(
      meta([
        call({ arguments: { a: 1 }, output: 'ok', association: 'turn' }),
        call({ error: 'boom', association: 'unknown' }),
        call(),
      ]),
    )!.steps
    expect(steps[0]!.sections).toEqual([
      { label: 'Arguments', raw: { a: 1 } },
      { label: 'Output', raw: 'ok' },
    ])
    expect(steps[0]!.association).toBe('turn')
    expect(steps[1]!.sections).toEqual([{ label: 'Error', raw: 'boom' }])
    expect(steps[1]!.association).toBe('unknown')
    expect(steps[2]!.sections).toEqual([])
    expect(steps[2]!.association).toBe('exact')
  })

  it('skips malformed entries with a warning naming the index and field, never the payload', () => {
    const warn = vi.fn()
    const got = toolCallsToSteps(
      meta([
        call({ name: 'ok' }),
        'nope',
        call({ name: '' }),
        call({ status: 7, output: 'SECRET' }),
        call({ name: 'fine', status: 'weird' }),
      ]),
      warn,
    )!
    expect(got.steps.map((s) => [s.name, s.recordedStatus])).toEqual([
      ['ok', 'succeeded'],
      ['fine', 'unknown'],
    ])
    expect(warn.mock.calls.map((c) => c[0])).toEqual([
      '[chat] recorded tool call 1: invalid entry; skipped',
      '[chat] recorded tool call 2: invalid name; skipped',
      '[chat] recorded tool call 3: invalid status; skipped',
    ])
    expect(JSON.stringify(warn.mock.calls)).not.toContain('SECRET')
  })

  it('no array of calls means no chips at all; the capture policy is carried', () => {
    for (const m of [
      null,
      {},
      { coding_agent: {} },
      { coding_agent: { tool_calls: 'x' } },
      meta({}),
    ])
      expect(toolCallsToSteps(m)).toBeNull()
    expect(toolCallsToSteps(meta([], 'metadata'))).toEqual({ steps: [], captured: false })
  })

  it('shows a string as given and JSON indented, cut at SAVED_DETAIL_MAX with the full text kept', () => {
    expect(sectionText('{"not":"parsed"}')).toEqual({
      text: '{"not":"parsed"}',
      cut: false,
      full: '{"not":"parsed"}',
    })
    expect(sectionText({ a: [1] }).text).toBe('{\n  "a": [\n    1\n  ]\n}')
    const long = 'x'.repeat(tuning.SAVED_DETAIL_MAX + 10)
    const t = sectionText(long)
    expect(t).toMatchObject({ cut: true, full: long })
    expect(t.text).toHaveLength(tuning.SAVED_DETAIL_MAX)
    expect([null, '', [], {}].every(isEmptyValue)).toBe(true)
    expect([0, 'x', [0], { a: 1 }].some(isEmptyValue)).toBe(false)
  })

  it('maps 2000 all-failed calls with 1 MB fields quickly (E8)', () => {
    const big = 'y'.repeat(1024 * 1024)
    const calls = Array.from({ length: 2000 }, (_, i) =>
      call({ id: `t${i}`, status: 'failed', arguments: big, output: big, error: big }),
    )
    const t0 = performance.now()
    const got = toolCallsToSteps(meta(calls))!
    expect(performance.now() - t0).toBeLessThan(50)
    expect(got.steps).toHaveLength(2000)
  })

  it('knows a metadata-only session, also once its harness agent is deleted (R-6, E16)', () => {
    const gone = { agent_id: null, agent_url: '/api/agents/deleted/a2a' }
    expect(
      isMetadataOnly({ ...gone, is_coding_agent: true, message_count: 0, title: 'Anything' }),
    ).toBe(true)
    expect(
      isMetadataOnly({
        ...gone,
        is_coding_agent: false,
        message_count: 0,
        title: 'Coding session',
      }),
    ).toBe(true)
    expect(
      isMetadataOnly({ ...gone, is_coding_agent: false, message_count: 0, title: 'Other' }),
    ).toBe(false)
    expect(
      isMetadataOnly({ ...gone, is_coding_agent: true, message_count: 4, title: 'Coding session' }),
    ).toBe(false)
    // A user can title any chat "Coding session": a direct or routed chat is never metadata-only.
    expect(
      isMetadataOnly({
        agent_id: 'a1',
        agent_url: '/api/agents/a1/a2a',
        is_coding_agent: false,
        message_count: 0,
        title: 'Coding session',
      }),
    ).toBe(false)
    expect(
      isMetadataOnly({
        agent_id: null,
        agent_url: null,
        is_coding_agent: false,
        message_count: 0,
        title: 'Coding session',
      }),
    ).toBe(false)
    expect(isMetadataOnly(undefined)).toBe(false)
  })
})

describe('the chips (test 4 component parts, E8)', () => {
  it('Copy full value copies the untruncated text', async () => {
    const writeText = vi.fn(() => Promise.resolve())
    vi.stubGlobal('navigator', { ...navigator, clipboard: { writeText } })
    const long = 'z'.repeat(tuning.SAVED_DETAIL_MAX + 500)
    render(
      <RecordedCalls calls={toolCallsToSteps(meta([call({ output: long })]))!} now={Date.now()} />,
    )
    await userEvent.click(screen.getByTestId('recorded-chip'))
    expect(
      screen.getByText(`Truncated. Showing the first ${tuning.SAVED_DETAIL_MAX} characters.`),
    ).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Copy full value' }))
    expect(writeText).toHaveBeenCalledWith(long)
  })

  it('2000 failed calls render the summary and at most 20 chips; expanding shows 50, then all', async () => {
    const calls = Array.from({ length: 2000 }, (_, i) =>
      call({ id: `t${i}`, name: `step${i}`, status: 'failed' }),
    )
    render(<RecordedCalls calls={toolCallsToSteps(meta(calls))!} now={Date.now()} />)
    expect(screen.getByTestId('calls-summary')).toHaveTextContent('2000 tool calls · 2000 failed')
    expect(screen.getAllByTestId('recorded-chip')).toHaveLength(20)
    expect(screen.getByText('+1980 more failed')).toBeInTheDocument()
    await userEvent.click(screen.getByTestId('calls-summary'))
    expect(screen.getAllByTestId('recorded-chip')).toHaveLength(50)
    await userEvent.click(screen.getByRole('button', { name: 'Show all 2000' }))
    expect(screen.getAllByTestId('recorded-chip')).toHaveLength(2000)
  })

  it('a policy other than content says Content not captured', async () => {
    render(
      <RecordedCalls
        calls={toolCallsToSteps(meta([call({ output: 'x' })], 'metadata'))!}
        now={Date.now()}
      />,
    )
    await userEvent.click(screen.getByTestId('recorded-chip'))
    expect(screen.getByTestId('recorded-detail')).toHaveTextContent('Content not captured')
  })
})

describe('a recorded chat (test 12)', () => {
  it('chips sit above the text; 6 calls collapse to "6 tool calls · 1 failed" with the failed chip after it', async () => {
    renderApp(`/chat/${RECORDED}`)
    const first = (await screen.findAllByTestId('recorded-calls'))[0]!
    const summary = within(first).getByTestId('calls-summary')
    expect(summary).toHaveTextContent('6 tool calls · 1 failed')
    const chips = within(first).getAllByTestId('recorded-chip')
    expect(chips.map((c) => c.textContent)).toEqual([expect.stringMatching(/run_tests· failed/)])
    // Above the assistant text.
    const reply = screen.getByText(/I split the handler into validation/)
    expect(first.compareDocumentPosition(reply) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    await userEvent.click(summary)
    expect(within(first).getAllByTestId('recorded-chip')).toHaveLength(6)
    expect(within(first).getByText('inferred')).toHaveAttribute(
      'title',
      'Matched to this turn by timing',
    )
  })

  it('a chip opens its sections as preformatted text below the chips; an empty section says No output', async () => {
    renderApp(`/chat/${RECORDED}`)
    const first = (await screen.findAllByTestId('recorded-calls'))[0]!
    await userEvent.click(within(first).getByTestId('recorded-chip'))
    const detail = within(first).getByTestId('recorded-detail')
    expect(
      within(detail).getByRole('region', { name: 'Arguments' }).querySelector('pre'),
    ).toHaveTextContent('"cmd": "npm test -- billing"')
    expect(
      within(detail).getByRole('region', { name: 'Error' }).querySelector('pre'),
    ).toHaveTextContent('Timeout: stripe-mock')
    // Detail sits below the chip list, not inside a chip.
    expect(within(first).getByRole('list', { name: 'Tool calls' }).contains(detail)).toBe(false)
    await userEvent.click(within(first).getByTestId('calls-summary'))
    await userEvent.click(
      within(first)
        .getAllByTestId('recorded-chip')
        .find((c) => /write_file/.test(c.textContent ?? ''))!,
    )
    expect(
      within(within(first).getByTestId('recorded-detail')).getByRole('region', { name: 'Output' }),
    ).toHaveTextContent('No output')
  })

  it('five calls show one by one with their status words; a cut value offers Copy full value; no composer', async () => {
    renderApp(`/chat/${RECORDED}`)
    const [, second, third] = await screen.findAllByTestId('recorded-calls')
    expect(within(second!).queryByTestId('calls-summary')).toBeNull()
    expect(
      within(second!)
        .getAllByTestId('recorded-chip')
        .map((c) => c.textContent),
    ).toEqual([
      expect.stringMatching(/bash· denied/),
      expect.stringMatching(/web_fetch· timed out/),
      expect.stringMatching(/grep· cancelled/),
      expect.stringMatching(/bash· No result recorded/),
      expect.stringMatching(/read_file· No result recorded/),
    ])
    expect(within(third!).getByText('Link unknown')).toBeInTheDocument()
    await userEvent.click(within(third!).getAllByTestId('recorded-chip')[0]!)
    expect(within(third!).getByRole('button', { name: 'Copy full value' })).toBeInTheDocument()
    expect(screen.queryByRole('textbox', { name: /^Message/ })).toBeNull()
    expect(screen.getByRole('link', { name: 'View in Harnesses →' })).toHaveAttribute(
      'href',
      '/harnesses',
    )
  })

  it('a metadata-only session shows why there is nothing to read', async () => {
    renderApp(`/chat/${METADATA_ONLY}`)
    const empty = await screen.findByTestId('metadata-only')
    expect(empty).toHaveTextContent(
      'Only metadata was recorded for this session. Turn on content capture in the harness integration to see messages.',
    )
    expect(within(empty).getByRole('link', { name: 'View in Harnesses →' })).toBeInTheDocument()
  })
})

describe("the rail's Chats and Recorded views (D1, DX-T1, DS1, DS12)", () => {
  it('under many-recorded, Chats has no live chat on page 1 and Recorded (50+) lists them across the page boundary', async () => {
    configureChatMock({ manyRecorded: true })
    renderApp('/chat')
    const rail = await screen.findByRole('navigation', { name: 'Chats' })
    expect(await within(rail).findByText('No live chats in the loaded list')).toBeInTheDocument()
    await userEvent.click(within(rail).getByRole('radio', { name: 'Recorded (50+)' }))
    await waitFor(() => expect(within(rail).getAllByTestId('rail-row')).toHaveLength(50))
    await userEvent.click(within(rail).getByRole('button', { name: 'Load more' }))
    await waitFor(() => expect(within(rail).getAllByTestId('rail-row')).toHaveLength(62))
    await userEvent.click(within(rail).getByRole('radio', { name: 'Chats' }))
    expect(within(rail).getAllByTestId('rail-row')).toHaveLength(4) // c001-c003 and the showcase chat (M2)
  })

  it('arrow keys switch views, and the choice is remembered per viewer', async () => {
    renderApp('/chat')
    const rail = await screen.findByRole('navigation', { name: 'Chats' })
    const chats = await within(rail).findByRole('radio', { name: 'Chats' })
    expect(chats).toHaveAttribute('aria-checked', 'true')
    chats.focus()
    // Chats → Waiting → Recorded: all three always show.
    await userEvent.keyboard('{ArrowRight}')
    expect(within(rail).getByRole('radio', { name: 'Waiting' })).toHaveAttribute(
      'aria-checked',
      'true',
    )
    await userEvent.keyboard('{ArrowRight}')
    expect(within(rail).getByRole('radio', { name: 'Recorded (2)' })).toHaveAttribute(
      'aria-checked',
      'true',
    )
    expect(within(rail).getByRole('radio', { name: 'Recorded (2)' })).toHaveFocus()
    expect(readRailView(ADMIN_ID)).toBe('recorded')
    document.body.innerHTML = ''
    renderApp('/chat')
    const again = await screen.findByRole('navigation', { name: 'Chats' })
    expect(await within(again).findByRole('radio', { name: 'Recorded (2)' })).toHaveAttribute(
      'aria-checked',
      'true',
    )
  })

  it('always shows Chats, Waiting and Recorded; an empty view says so (user decision, 2026-09-28)', async () => {
    renderApp('/chat')
    const rail = await screen.findByRole('navigation', { name: 'Chats' })
    await waitFor(() =>
      expect(
        within(rail)
          .getAllByRole('radio')
          .map((r) => r.getAttribute('aria-label')),
      ).toEqual(['Chats', 'Waiting', 'Recorded (2)']),
    )
    await userEvent.click(within(rail).getByRole('radio', { name: 'Waiting' }))
    expect(await within(rail).findByText(copy.noWaiting)).toBeInTheDocument()
  })

  it('opening a recorded chat shows the Recorded view, so the active row is in the list', async () => {
    rememberRailView(ADMIN_ID, 'chats')
    renderApp(`/chat/${RECORDED}`)
    const rail = await screen.findByRole('navigation', { name: 'Chats' })
    await waitFor(() =>
      expect(within(rail).getByRole('radio', { name: 'Recorded (2)' })).toHaveAttribute(
        'aria-checked',
        'true',
      ),
    )
    expect(
      within(rail)
        .getAllByTestId('rail-row')
        .find((r) => r.getAttribute('aria-current') === 'page'),
    ).toBeTruthy()
  })

  it('the remembered view is cleared with the drafts on sign-out', () => {
    rememberRailView('u1', 'recorded')
    clearDrafts('u1')
    expect(readRailView('u1')).toBeNull()
  })
})

describe('axe (test 16, M1 screens)', () => {
  it('a recorded chat with a detail open, and the Recorded view', async () => {
    renderApp(`/chat/${RECORDED}`)
    const first = (await screen.findAllByTestId('recorded-calls'))[0]!
    await userEvent.click(within(first).getByTestId('recorded-chip'))
    const result = await axe.run(document.body, {
      rules: { 'color-contrast': { enabled: false }, region: { enabled: false } },
    })
    expect(
      result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(' ')).join(', ')}`),
    ).toEqual([])
  })
})
