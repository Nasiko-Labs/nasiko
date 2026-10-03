import { describe, expect, it } from 'vitest'
import { artifact, CHAT_SCENARIOS, dataPart, status, type ChatScenario } from '@/mocks/chat'
import {
  applyEvent,
  classifyFrame,
  emptyTurn,
  normalizeState,
  reduceSseEvent,
  replyText,
  type TurnState,
} from './a2aReducer'
import liveFailed from './__fixtures__/live-direct-agent-failed.json'
import liveSimulated from './__fixtures__/live-direct-simulated.json'
import { outcomeOf } from './send'

const run = (scenario: ChatScenario): TurnState =>
  CHAT_SCENARIOS[scenario].reduce(
    (s, f) =>
      reduceSseEvent(s, { event: f.event ?? 'message', data: f.raw ?? JSON.stringify(f.data) })
        .state,
    emptyTurn(),
  )

describe('normalizeState', () => {
  it.each([
    ['TASK_STATE_COMPLETED', 'completed'],
    ['completed', 'completed'],
    ['input-required', 'input_required'],
    ['TASK_STATE_INPUT_REQUIRED', 'input_required'],
    ['cancelled', 'canceled'],
    ['weird', 'unknown'],
    [undefined, 'unknown'],
  ])('%s → %s', (raw, want) => expect(normalizeState(raw)).toBe(want))
})

describe('classifyFrame shapes', () => {
  it('reads 1.0 frames bare and under result', () => {
    const bare = classifyFrame(artifact('hi'))
    const wrapped = classifyFrame({ jsonrpc: '2.0', result: artifact('hi') })
    expect(bare).toEqual(wrapped)
    expect(bare[0]).toMatchObject({ type: 'artifact', text: 'hi', append: false })
  })

  it('reads 0.3 artifact-update with final as lastChunk', () => {
    const ev = classifyFrame({
      result: {
        kind: 'artifact-update',
        artifact: { artifactId: 'z', parts: [{ kind: 'text', text: 'x' }] },
        append: true,
        final: true,
      },
    })
    expect(ev[0]).toMatchObject({
      type: 'artifact',
      artifactId: 'z',
      append: true,
      lastChunk: true,
    })
  })

  it('reads 0.3 status-update', () => {
    const ev = classifyFrame({
      result: {
        kind: 'status-update',
        status: { state: 'working', message: { parts: [{ kind: 'text', text: 'thinking' }] } },
      },
    })
    expect(ev).toEqual([{ type: 'status', state: 'working', text: 'thinking' }])
  })

  it('reads JSON-RPC errors', () => {
    expect(
      classifyFrame({
        jsonrpc: '2.0',
        id: null,
        error: { code: -32603, message: 'internal error' },
      }),
    ).toEqual([{ type: 'error', message: 'internal error', code: -32603 }])
  })

  it('reads Nasiko data parts', () => {
    const ev = classifyFrame(
      status('TASK_STATE_WORKING', [
        dataPart({ type: 'usage_meta', duration_ms: 10, cost_usd: '0.5', input_tokens: 3 }),
        dataPart({ type: 'unknown_thing' }),
      ]),
    )
    expect(ev[0]).toEqual({
      type: 'usage',
      usage: { duration_ms: 10, cost_usd: 0.5, input_tokens: 3 },
    })
    expect(ev).toHaveLength(2)
  })

  it('ignores a malformed hitl part', () => {
    expect(
      classifyFrame(status('TASK_STATE_WORKING', [dataPart({ type: 'hitl', kind: 'nope' })])),
    ).toEqual([{ type: 'status', state: 'working', text: '' }])
  })

  it('ignores unknown shapes', () => {
    expect(classifyFrame({ whatever: true })).toEqual([])
    expect(classifyFrame('text')).toEqual([])
  })
})

describe('scenarios', () => {
  it('direct-plain: reply, trace and usage', () => {
    const s = run('direct-plain')
    expect(replyText(s)).toBe('Hello from the agent.')
    expect(s.traceId).toBe('5eedc0000000000000000000000000a1')
    expect(s.usage).toMatchObject({ input_tokens: 812, cost_usd: 0.0021 })
  })

  it('terminal → usage → terminal keeps the usage that arrived after the first completion', () => {
    const s = run('terminal-usage-terminal')
    expect(replyText(s)).toBe('Done.')
    expect(s.usage?.duration_ms).toBe(1240)
    expect(s.taskState).toBe('completed')
  })

  it.each([
    ['a2a03-message', 'A 0.3 message reply.'],
    ['a2a03-task', 'A 0.3 task reply.'],
    ['a2a10-task', 'A 1.0 task reply.'],
    ['lowercase-states', 'lower'],
  ] as const)('%s', (name, text) => {
    expect(replyText(run(name))).toBe(text)
  })

  it('append:false resets the artifact, even when empty', () => {
    expect(replyText(run('append-reset'))).toBe('final')
  })

  it('multi-artifact replies concatenate in arrival order', () => {
    expect(replyText(run('multi-artifact'))).toBe('Part one. Part two.')
  })

  it('tool_call and tool_result merge into one step', () => {
    const s = run('direct-steps')
    expect(s.steps).toEqual([
      {
        key: 'search_docs#1',
        kind: 'agent',
        name: 'search_docs',
        status: 'ok',
        durationMs: 1200,
        detail: '3 hits',
      },
    ])
  })

  it('a pause records the request and no reply', () => {
    const s = run('hitl-options')
    expect(s.request).toMatchObject({
      id: '5eedc000-0000-4000-8000-00000000a001',
      kind: 'input_required',
    })
    expect(s.taskState).toBe('working')
    expect(replyText(s)).toBe('')
  })

  it('a failed turn keeps the error text', () => {
    expect(run('failed').error).toEqual({ message: 'upstream timeout' })
  })

  it('counts unreadable frames and keeps going', () => {
    const s = run('stream-garbage')
    expect(s.badFrames).toBe(1)
    expect(replyText(s)).toBe('still here')
  })

  it('reads the event: error frame', () => {
    const r = reduceSseEvent(emptyTurn(), {
      event: 'error',
      data: '{"error":"could not persist request"}',
    })
    expect(r.state.error).toEqual({ message: 'could not persist request', code: undefined })
  })

  it('never mutates its input', () => {
    const s = emptyTurn()
    const frozen = JSON.stringify(s)
    applyEvent(s, { type: 'artifact', artifactId: 'a', text: 'x', append: false, lastChunk: false })
    expect(JSON.stringify(s)).toBe(frozen)
  })
})

/** Streams recorded from nasiko-server cb3aaf0c on 2026-09-27 (plan §12 replay fixtures). */
describe('live replay', () => {
  const replay = (frames: string[]) =>
    frames.reduce((s, data) => reduceSseEvent(s, { event: 'message', data }).state, emptyTurn())

  it('simulated-agent: one artifact per chunk joins into one reply, the terminal echo is not doubled', () => {
    const s = replay(liveSimulated.frames)
    const out = outcomeOf(s, false)
    expect(out.kind).toBe('save')
    const body = (out as Extract<typeof out, { kind: 'save' }>).body
    const echo = (
      JSON.parse(liveSimulated.frames[31]!) as {
        result: { statusUpdate: { status: { message: { parts: { text: string }[] } } } }
      }
    ).result.statusUpdate.status.message.parts[0]!.text
    expect(body.content.trim()).toBe(echo.trim())
    expect(body.content).not.toContain('thinking...')
    expect(body.usage).toEqual({ duration_ms: 4300, trace_id: '4d5a394be0e3a1a9e1424fc1236fa00b' })
    expect(s.badFrames).toBe(0)
  })

  it('an agent task that fails is a failure even though the server closes its own task as completed', () => {
    const s = replay(liveFailed.frames)
    expect(outcomeOf(s, false).kind).toBe('agent-failed')
    expect(s.traceId).toBe('4a4338a5718c9c042ffcad62c6321d70')
  })
})
