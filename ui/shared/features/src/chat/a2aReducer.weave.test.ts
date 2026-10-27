import { describe, expect, it } from 'vitest'
import { emptyTurn, reduceSseEvent, replyText, type TurnState } from './a2aReducer'

type Json = Record<string, unknown>

const statusFrame = (data: Json, state = 'working'): Json => ({
  jsonrpc: '2.0',
  id: '1',
  result: { kind: 'status-update', status: { state, message: { parts: [{ data }] } } },
})

const chunk = (
  text: string,
  opts: { append?: boolean; last?: boolean; id?: string } = {},
): Json => ({
  jsonrpc: '2.0',
  id: '1',
  result: {
    kind: 'artifact-update',
    artifact: { artifactId: opts.id ?? 'reply', parts: [{ text }] },
    append: opts.append ?? true,
    lastChunk: opts.last ?? false,
    final: opts.last ?? false,
  },
})

const toolCall = (over: Json = {}): Json =>
  statusFrame({
    type: 'tool_call',
    tool: 'web_search',
    tool_name: 'web_search',
    message: 'rust latest',
    arguments: { query: 'rust latest' },
    id: 'c1',
    ...over,
  })

const toolResult = (over: Json = {}): Json =>
  statusFrame({
    type: 'tool_result',
    tool: 'web_search',
    tool_name: 'web_search',
    result: 'found it',
    content: 'found it',
    success: true,
    duration_ms: 42,
    id: 'c1',
    ...over,
  })

const feed = (frames: Json[], from: TurnState = emptyTurn()): TurnState =>
  frames.reduce(
    (s, f) => reduceSseEvent(s, { event: 'update', data: JSON.stringify(f) }).state,
    from,
  )

describe('Weave tool frames', () => {
  it('renders a tool_call as a running tool step named after the tool', () => {
    const s = feed([toolCall()])
    expect(s.steps).toHaveLength(1)
    expect(s.steps[0]).toMatchObject({ kind: 'tool', name: 'web_search', status: 'running' })
  })

  it('shows the search query as the step detail', () => {
    expect(feed([toolCall()]).steps[0].detail).toBe('rust latest')
  })

  it('does not treat the tool as an agent', () => {
    expect(feed([toolCall()]).steps.some((x) => x.kind === 'agent')).toBe(false)
  })

  it('leaves via unset for a top-level call', () => {
    expect(feed([toolCall()]).steps[0].via).toBeUndefined()
  })

  it('sets via when a sub-agent relayed the call', () => {
    const s = feed([toolCall({ via_agent: 'slack_worker' })])
    expect(s.steps[0]).toMatchObject({ kind: 'tool', name: 'web_search', via: 'slack_worker' })
  })

  it('ignores an empty via_agent', () => {
    expect(feed([toolCall({ via_agent: '' })]).steps[0].via).toBe('')
  })

  it('closes the same step with ok when the result succeeds', () => {
    const s = feed([toolCall(), toolResult()])
    expect(s.steps).toHaveLength(1)
    expect(s.steps[0]).toMatchObject({ status: 'ok', durationMs: 42, detail: 'found it' })
  })

  it('closes the same step as an error when the result has success false', () => {
    const s = feed([toolCall(), toolResult({ success: false, result: 'boom' })])
    expect(s.steps).toHaveLength(1)
    expect(s.steps[0]).toMatchObject({ kind: 'tool', status: 'error', detail: 'boom' })
  })

  it('treats a result without a success field as ok', () => {
    const s = feed([toolCall(), toolResult({ success: undefined })])
    expect(s.steps[0].status).toBe('ok')
  })

  it('keeps the call detail when the result carries none', () => {
    const s = feed([toolCall(), toolResult({ result: undefined })])
    expect(s.steps[0].detail).toBe('rust latest')
  })

  it('pairs a relayed result with its relayed call', () => {
    const s = feed([
      toolCall({ via_agent: 'slack_worker' }),
      toolResult({ via_agent: 'slack_worker', success: false }),
    ])
    expect(s.steps).toHaveLength(1)
    expect(s.steps[0]).toMatchObject({ via: 'slack_worker', status: 'error' })
  })

  it('shows an unpaired failed result as its own error step', () => {
    const s = feed([toolResult({ success: false })])
    expect(s.steps).toEqual([
      expect.objectContaining({ kind: 'tool', name: 'web_search', status: 'error' }),
    ])
  })

  it('keeps two parallel calls to the same tool as two steps', () => {
    const s = feed([toolCall({ id: 'c1' }), toolCall({ id: 'c2', message: 'second' })])
    expect(s.steps).toHaveLength(2)
  })

  it('closes the right call when parallel results arrive out of order', () => {
    const s = feed([
      toolCall({ id: 'c1', message: 'first' }),
      toolCall({ id: 'c2', message: 'second' }),
      toolResult({ id: 'c2', success: false, result: 'second failed' }),
    ])
    const byDetail = Object.fromEntries(s.steps.map((x) => [x.detail, x.status]))
    expect(byDetail['first']).toBe('running')
    expect(byDetail['second failed']).toBe('error')
  })

  it('keeps tool steps out of the reply text', () => {
    const s = feed([toolCall(), toolResult(), chunk('Hi')])
    expect(replyText(s)).toBe('Hi')
  })

  it('does not break on a tool_call without a message', () => {
    const s = feed([toolCall({ message: undefined })])
    expect(s.steps[0].status).toBe('running')
  })

  it('falls back to a generic name when a call has no tool', () => {
    const s = feed([statusFrame({ type: 'tool_call', id: 'c9' })])
    expect(s.steps[0]).toMatchObject({ kind: 'tool', name: 'tool' })
  })

  it('does not set a non-string duration', () => {
    const s = feed([toolCall(), toolResult({ duration_ms: 'soon' })])
    expect(s.steps[0].durationMs).toBeUndefined()
  })
})

describe('Weave live reply artifacts', () => {
  it('appends streamed chunks into one reply artifact', () => {
    const s = feed([chunk('Hel'), chunk('lo')])
    expect(s.artifacts).toEqual([{ id: 'reply', text: 'Hello' }])
  })

  it('does not wipe the reply when the closing empty append chunk arrives', () => {
    const s = feed([chunk('Hel'), chunk('lo'), chunk('', { last: true })])
    expect(replyText(s)).toBe('Hello')
  })

  it('creates the artifact from a first chunk marked append', () => {
    expect(feed([chunk('Hi')]).artifacts).toEqual([{ id: 'reply', text: 'Hi' }])
  })

  it('shows an empty reply when only the closing chunk arrives', () => {
    const s = feed([chunk('', { last: true })])
    expect(replyText(s)).toBe('')
  })

  it('sends a no-delta reply once and keeps it after the closing chunk', () => {
    const s = feed([chunk('Whole answer'), chunk('', { last: true })])
    expect(replyText(s)).toBe('Whole answer')
  })

  it('keeps multibyte text split across chunks intact', () => {
    const s = feed([
      chunk('caf'),
      chunk('é '),
      chunk('\u{1F468}‍'),
      chunk('\u{1F469}'),
      chunk('', { last: true }),
    ])
    expect(replyText(s)).toBe('café \u{1F468}‍\u{1F469}')
  })

  it('keeps whitespace-only chunks that sit between words', () => {
    const s = feed([chunk('a'), chunk(' '), chunk('b')])
    expect(replyText(s)).toBe('a b')
  })

  it('keeps the reply when tool and thinking frames arrive between chunks', () => {
    const s = feed([
      chunk('before '),
      statusFrame({ type: 'thinking', content: 'hmm' }),
      toolCall(),
      toolResult(),
      chunk('after'),
      chunk('', { last: true }),
    ])
    expect(replyText(s)).toBe('before after')
  })

  it('replaces the artifact for a non-live final frame without append', () => {
    const s = feed([
      chunk('draft', { append: false }),
      chunk('full text', { append: false, last: true }),
    ])
    expect(replyText(s)).toBe('full text')
  })

  it('files a final frame with no artifactId under one default artifact', () => {
    const s = feed([
      {
        jsonrpc: '2.0',
        id: '1',
        result: {
          kind: 'artifact-update',
          artifact: { parts: [{ text: 'done' }] },
          append: false,
          final: true,
        },
      },
    ])
    expect(s.artifacts).toEqual([{ id: 'default', text: 'done' }])
  })

  it('does not let a thinking frame leak into the reply', () => {
    const s = feed([statusFrame({ type: 'thinking', content: 'secret' }), chunk('Answer')])
    expect(replyText(s)).toBe('Answer')
  })

  it('survives a skill_meta frame', () => {
    const s = feed([statusFrame({ type: 'skill_meta', catalogVersion: 'abc' }), chunk('x')])
    expect(replyText(s)).toBe('x')
    expect(s.badFrames).toBe(0)
  })

  it('does not mutate the previous state', () => {
    const before = feed([chunk('a')])
    const snapshot = JSON.stringify(before)
    feed([chunk('b')], before)
    expect(JSON.stringify(before)).toBe(snapshot)
  })
})

describe('Weave thinking frames', () => {
  it('renders a thinking frame as activity', () => {
    const s = feed([statusFrame({ type: 'thinking', content: 'Checking the data.' })])
    expect(s.activity).toEqual(['Checking the data.'])
  })

  it('keeps thinking frames in arrival order', () => {
    const s = feed([
      statusFrame({ type: 'thinking', content: 'one' }),
      statusFrame({ type: 'thinking', content: 'two' }),
    ])
    expect(s.activity).toEqual(['one', 'two'])
  })

  it('ignores a thinking frame with empty content', () => {
    expect(feed([statusFrame({ type: 'thinking', content: '' })]).activity).toEqual([])
  })

  it('ignores a thinking frame whose content is not a string', () => {
    expect(feed([statusFrame({ type: 'thinking', content: 5 })]).activity).toEqual([])
  })

  it('does not create a step for thinking', () => {
    expect(feed([statusFrame({ type: 'thinking', content: 'x' })]).steps).toEqual([])
  })
})

describe('Weave failures', () => {
  it('marks the turn failed and keeps the streamed text when a failed frame follows chunks', () => {
    const s = feed([
      chunk('partial'),
      statusFrame(
        { type: 'sub_content', agent: 'weave', content: 'model exploded', code: 'rate_limited' },
        'failed',
      ),
    ])
    expect(s.taskState).toBe('failed')
    expect(s.error).not.toBeNull()
    expect(replyText(s)).toBe('partial')
  })

  it('surfaces the failure content rather than a bare state name', () => {
    const s = feed([
      statusFrame(
        { type: 'sub_content', agent: 'weave', content: 'model exploded', code: 'rate_limited' },
        'failed',
      ),
    ])
    expect(s.error?.message).toContain('model exploded')
  })
})
