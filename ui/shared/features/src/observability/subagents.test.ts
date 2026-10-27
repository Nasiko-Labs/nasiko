import { describe, expect, it } from 'vitest'
import { childRun, costWithSubagents, detailSubagents, rowSubagents } from './sessions'
import type { SessionDetail, SessionSummary } from './types'

const row = (extra: Record<string, unknown> = {}) =>
  ({
    id: 's1',
    session_id: 's1',
    agent_id: 'admin-claude-code',
    session_annotations: [],
    session_annotation_summaries: [],
    failed_flows: 0,
    num_traces: 3,
    start_time: '2026-10-20T10:00:00.000Z',
    duration_ms: 60_000,
    token_usage: { total: 1_200 },
    cost_summary: { total: { cost: 0.5 } },
    ...extra,
  }) as SessionSummary

const detail = (extra: Record<string, unknown> = {}) =>
  ({ session_id: 's1', traces: [], ...extra }) as unknown as SessionDetail

describe('rowSubagents', () => {
  it('an older server (no R15 fields) reads as no subagents and no parent', () => {
    expect(rowSubagents(row())).toEqual({
      parentSessionId: null,
      spawnToolCallId: null,
      subagentType: null,
      count: 0,
      totals: null,
    })
  })

  it('reads the count and the children totals, separate from the row', () => {
    const r = rowSubagents(
      row({
        parent_session_id: null,
        subagent_count: 2,
        subagents: { turns: 5, tokens: 28_200, cost: 0.1254 },
      }),
    )
    expect(r.count).toBe(2)
    expect(r.totals).toEqual({ turns: 5, tokens: 28_200, cost: 0.1254 })
  })

  it('a malformed field falls back on its own, never failing the row', () => {
    const r = rowSubagents(
      row({ subagent_count: -1, subagents: 'x', parent_session_id: 42, subagent_type: 'Explore' }),
    )
    expect(r).toMatchObject({
      count: 0,
      totals: null,
      parentSessionId: null,
      subagentType: 'Explore',
    })
  })
})

describe('detailSubagents', () => {
  it('lists each child with its duration; a bad entry drops itself', () => {
    const d = detailSubagents(
      detail({
        subagents: [
          {
            session_id: 'c1',
            subagent_type: 'Explore',
            spawn_tool_call_id: 'call-1',
            turns: 3,
            tokens: 100,
            cost: 0.01,
            start_time: '2026-10-20T10:00:00.000Z',
            end_time: '2026-10-20T10:01:30.000Z',
          },
          { subagent_type: 'no id' },
        ],
      }),
    )
    expect(d.subagents).toEqual([
      {
        sessionId: 'c1',
        type: 'Explore',
        spawnToolCallId: 'call-1',
        turns: 3,
        tokens: 100,
        cost: 0.01,
        startTime: '2026-10-20T10:00:00.000Z',
        durationMs: 90_000,
      },
    ])
  })

  it('a child names its parent; an older server names nothing', () => {
    expect(
      detailSubagents(detail({ parent_session_id: 'p1', subagent_type: 'Plan' })),
    ).toMatchObject({ parentSessionId: 'p1', subagentType: 'Plan', subagents: [] })
    expect(detailSubagents(detail())).toEqual({
      parentSessionId: null,
      spawnToolCallId: null,
      subagentType: null,
      subagents: [],
    })
  })
})

describe('childRun', () => {
  it("a ?parent= row's own figures are the run's", () => {
    expect(childRun(row({ subagent_type: 'Explore', spawn_tool_call_id: 'call-1' }))).toMatchObject(
      { sessionId: 's1', type: 'Explore', turns: 3, tokens: 1_200, cost: 0.5 },
    )
  })
})

describe('costWithSubagents', () => {
  it('adds the subagents to a known own cost, shows them alone otherwise, and nothing without them', () => {
    expect(costWithSubagents(0.5, 0.25)).toEqual({ kind: 'incl', value: 0.75 })
    expect(costWithSubagents(null, 0.25)).toEqual({ kind: 'only', value: 0.25 })
    expect(costWithSubagents(0.5, null)).toBeNull()
  })
})
