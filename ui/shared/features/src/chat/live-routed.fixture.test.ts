/**
 * The routed fixture's scrub is an allowlist (NE-6): every string in the recorded frames sits under
 * a key the smoke script keeps verbatim, is a placeholder the script wrote, or is empty. A string
 * anywhere else is agent or LLM text that escaped the scrub.
 */
import { describe, expect, it } from 'vitest'
import {
  KEEP,
  KEY_SHAPE,
  PLACEHOLDER,
  SECRET,
  scrub,
} from '../../../../scripts/smoke-chat-routed.ts'
import fixture from './__fixtures__/live-routed.json'

/** Every string value with the key it sits under (array items take their array's key). */
function strings(v: unknown, key = '', path = ''): { key: string; path: string; value: string }[] {
  if (typeof v === 'string') return [{ key, path, value: v }]
  if (Array.isArray(v)) return v.flatMap((x, i) => strings(x, key, `${path}[${i}]`))
  if (v && typeof v === 'object')
    return Object.entries(v).flatMap(([k, x]) => strings(x, k, `${path}.${k}`))
  return []
}

describe('live routed fixture scrub', () => {
  it('keeps only allowlisted strings, placeholders or empty strings in the frames', () => {
    const leaks = strings(fixture.frames, '', 'frames').filter(
      ({ key, value }) => value !== '' && !KEEP.has(key) && !PLACEHOLDER.test(value),
    )
    expect(leaks).toEqual([])
  })

  it('keeps only identifier-shaped object keys (a key can be agent text too)', () => {
    const keys = (v: unknown): string[] =>
      Array.isArray(v)
        ? v.flatMap(keys)
        : v && typeof v === 'object'
          ? Object.entries(v).flatMap(([k, x]) => [k, ...keys(x)])
          : []
    expect(keys(fixture.frames).filter((k) => !KEY_SHAPE.test(k))).toEqual([])
    const [out] = scrub([{ type: 'x', data: { 'Jane Doe <jane@example.com>': 'hi', turn: 1 } }], {
      sessionId: 's',
      traceId: 't',
    }) as [{ data: Record<string, unknown> }]
    expect(Object.keys(out.data)).toEqual(['k-1', 'turn'])
  })

  it('has no credential-looking text anywhere', () => {
    expect(SECRET.test(JSON.stringify(fixture))).toBe(false)
  })

  it('carries only the metadata the script writes at the top level', () => {
    expect(Object.keys(fixture).sort()).toEqual([
      'expectedAgent',
      'frames',
      'prompt',
      'recorded',
      'server',
    ])
  })

  it('scrubs an unknown field and keeps structural ones', () => {
    const session = '0b7c3f6e-2d6a-4c1e-9d55-6a1f0c2b9e11'
    const trace = '4bf92f3577b34da6a3ce929d0e0e4736'
    const [out] = scrub(
      [
        {
          type: 'x',
          agent: 'currency-agent',
          taskId: session,
          trace_id: trace,
          detail: 'secret agent output',
          turn: 2,
          ok: true,
          empty: '',
          nested: [{ note: 'hi' }],
        },
      ],
      { sessionId: session, traceId: trace },
    )
    expect(out).toEqual({
      type: 'x',
      agent: 'currency-agent',
      taskId: 'session-1',
      trace_id: 'trace-1',
      detail: '<detail 19 chars>',
      turn: 2,
      ok: true,
      empty: '',
      nested: [{ note: '<note 2 chars>' }],
    })
  })

  it('drops credential headers and replaces raw ids inside kept strings', () => {
    const [out] = scrub(
      [
        {
          token: 'abc',
          Authorization: 'Bearer x',
          id: 'task 11111111-2222-3333-4444-555555555555',
        },
      ],
      { sessionId: 'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee', traceId: null },
    )
    expect(out).toEqual({ id: 'task id-2' })
  })
})
