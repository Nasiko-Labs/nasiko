// Ported from the React migration's domain/workflows/{model,editor}.test.ts, plus the lab's `main` rules.
import { describe, expect, it } from 'vitest'
import type { Agent } from '@/features/agents/types'
import type { HitlDto } from '@/features/chat/types'
import { ApiError } from '@/lib/api/client'
import {
  agentOptions,
  agentsOf,
  blankStep,
  execStatus,
  filterRuns,
  fromMaf,
  hasMetrics,
  hitlByStep,
  isDeployed,
  isDraftsAbsent,
  isExecActive,
  isOrphan,
  lastRun,
  moveStep,
  planFailure,
  planningTokens,
  remapStepError,
  runStepsLabel,
  runTitle,
  showsRunError,
  snapshot,
  stepStatus,
  toPayload,
  type EditorStep,
} from './logic'
import type { Execution, ExecutionRow, StepResult, WorkflowRow } from './types'

const wf = (over: Partial<WorkflowRow>): WorkflowRow => ({
  id: 'w',
  name: 'W',
  description: null,
  maf_json: null,
  ...over,
})
const step = (i: number, status: string, over: Partial<StepResult> = {}): StepResult => ({
  step_id: `s${i}`,
  step_index: i,
  status,
  ...over,
})
const exec = (over: Partial<ExecutionRow>): ExecutionRow => ({
  id: 'e',
  execution_number: 7,
  maf_id: 'w',
  status: 'success',
  created_at: new Date().toISOString(),
  workflow_name: 'W',
  ...over,
})
const err = (status: number, body: unknown = null) => new ApiError(status, body, '/x', 'x')

describe('status vocabulary', () => {
  it('names every run status, and the raw word for an unknown one', () => {
    expect(execStatus('running')).toEqual({ label: 'Running…', tone: 'info' })
    expect(execStatus('awaiting_human')).toEqual({ label: 'Awaiting action', tone: 'warning' })
    expect(execStatus('stopped')).toEqual({ label: 'Stopped', tone: 'neutral' })
    expect(execStatus('pending').label).toBe('Queued')
    expect(execStatus('weird')).toEqual({ label: 'weird', tone: 'neutral' })
  })
  it('a step that has not started is Pending, never Queued', () => {
    expect(stepStatus('pending').label).toBe('Pending')
    expect(stepStatus('success')).toEqual({ label: 'Complete', tone: 'success' })
  })
  it('keeps polling while paused', () => {
    expect(['pending', 'running', 'awaiting_human'].every(isExecActive)).toBe(true)
    expect(['success', 'failed', 'stopped', undefined].some(isExecActive)).toBe(false)
  })
  it('a missing status is deployed', () => {
    expect(isDeployed({})).toBe(true)
    expect(isDeployed({ status: 'draft' })).toBe(false)
  })
})

describe('lastRun', () => {
  it('reads the row, not a join of recent runs', () => {
    expect(lastRun(wf({ execution_count: 0 }))?.text).toBe('Not run yet')
    expect(lastRun(wf({ execution_count: 2, last_run_status: null }))?.text).toBe('Queued')
    expect(
      lastRun(wf({ execution_count: 2, last_run_status: 'success', last_run_at: 't' })),
    ).toEqual({ text: 'Last run succeeded', at: 't', tone: 'success' })
    // Legacy said "Running now" for these two.
    expect(lastRun(wf({ execution_count: 1, last_run_status: 'stopped' }))?.text).toBe(
      'Last run stopped',
    )
    expect(lastRun(wf({ execution_count: 1, last_run_status: 'awaiting_human' }))?.tone).toBe(
      'warning',
    )
    expect(lastRun(wf({ execution_count: 1, last_run_status: 'running' }))?.text).toBe(
      'Running now',
    )
  })
  it('says nothing on `main`, whose rows have no last-run fields (W-3)', () => {
    expect(lastRun(wf({ execution_count: 3 }))).toBeNull()
    expect(hasMetrics([wf({ execution_count: 3 })])).toBe(false)
    expect(hasMetrics([wf({ health: 'unknown' })])).toBe(true)
  })
  it('derives the agents from the steps when the row has no agent_names', () => {
    const steps = ['a', 'b', 'a'].map((n, i) => ({
      step_id: `${i}`,
      step_index: i,
      agent_id: n,
      agent_name: n,
      task_description: 't',
    }))
    expect(agentsOf(wf({ maf_json: { steps } }))).toEqual(['a', 'b'])
    expect(agentsOf(wf({ agent_names: ['x'] }))).toEqual(['x'])
  })
})

describe('filterRuns', () => {
  const now = Date.parse('2026-09-27T12:00:00Z')
  const ago = (days: number) => new Date(now - days * 86_400_000).toISOString()
  const rows = [
    exec({ id: '1', status: 'awaiting_human', created_at: ago(0), workflow_name: 'Pipeline 1' }),
    exec({ id: '2', status: 'running', created_at: ago(0), workflow_name: 'Pipeline 2' }),
    exec({ id: '3', status: 'success', created_at: ago(3), workflow_name: 'Pipeline 3' }),
    exec({ id: '4', status: 'failed', created_at: ago(10), workflow_name: 'Pipeline 4' }),
    exec({ id: '5', status: 'stopped', created_at: ago(60), workflow_name: null }),
  ]
  const ids = (f: Partial<Parameters<typeof filterRuns>[1]>) =>
    filterRuns(rows, { q: '', status: 'all', age: 'any', ...f }, now).map((r) => r.id)
  it('buckets statuses as the menu says', () => {
    expect(ids({ status: 'attention' })).toEqual(['1'])
    expect(ids({ status: 'running' })).toEqual(['2'])
    expect(ids({ status: 'failed' })).toEqual(['4', '5'])
  })
  it('windows by age', () => {
    expect(ids({ age: '7d' })).toEqual(['1', '2', '3'])
    expect(ids({ age: '1d' })).toEqual(['1', '2'])
  })
  it('searches the title as shown, "Deleted workflow" included', () => {
    expect(ids({ q: ' pipeline 3 ' })).toEqual(['3'])
    expect(ids({ q: 'deleted' })).toEqual(['5'])
    expect(runTitle(rows[4]!)).toBe('Deleted workflow #7')
  })
  it('a run of a deleted (or gone) workflow is an orphan', () => {
    expect(isOrphan(rows[4]!)).toBe(true)
    expect(isOrphan(exec({ workflow_status: 'deleted' }))).toBe(true)
    expect(isOrphan(exec({ maf_id: null }))).toBe(true)
    expect(isOrphan(exec({ workflow_status: 'active' }))).toBe(false)
  })
})

describe('run helpers', () => {
  it('counts steps without overflowing during the final synthesis', () => {
    const done = [step(0, 'success'), step(1, 'success')]
    expect(runStepsLabel(exec({ status: 'running', step_results: done }))).toBe('Step 2/2')
    expect(
      runStepsLabel(
        exec({ status: 'running', step_results: [step(0, 'success'), step(1, 'running')] }),
      ),
    ).toBe('Step 2/2')
    expect(runStepsLabel(exec({ status: 'success', step_results: done }))).toBe('2 steps')
    expect(runStepsLabel(exec({ step_results: null }))).toBeNull()
  })
  it('an error shows only once the run has stopped moving', () => {
    const e = (status: string): Execution => exec({ status, error: 'boom' })
    expect(showsRunError(e('failed'))).toBe(true)
    expect(showsRunError(e('stopped'))).toBe(true)
    // A retry is queued as pending with the last attempt's error.
    expect(showsRunError(e('pending'))).toBe(false)
  })
  it('reconciles the header total with the step chips', () => {
    expect(planningTokens(500, [step(0, 'success', { tokens_used: 100 }), step(1, 'x')])).toBe(400)
  })
  it('knows the planner refusals', () => {
    expect(planFailure(err(503))).toBe('no-key')
    expect(planFailure(err(400))).toBe('no-agents')
    expect(planFailure(err(422))).toBe('planner')
    expect(planFailure(err(429))).toBe('other')
    expect(planFailure(new Error('x'))).toBe('other')
  })
})

describe('isDraftsAbsent (W-1)', () => {
  it("reads `main`'s answers to the drafts routes as absent", () => {
    // /maf/workflow/drafts hits /maf/workflow/{id}: axum's UUID rejection is plain text.
    expect(isDraftsAbsent(err(400, 'Invalid URL: UUID parsing failed'))).toBe(true)
    expect(isDraftsAbsent(err(405))).toBe(true)
    expect(isDraftsAbsent(err(404))).toBe(true)
  })
  it("never mistakes a MAF handler's own error (the JSON envelope) for a missing route", () => {
    const env = { data: null, status_code: 400, message: 'instruction is required' }
    expect(isDraftsAbsent(err(400, env))).toBe(false)
    expect(isDraftsAbsent(err(404, { ...env, status_code: 404, message: 'draft not found' }))).toBe(
      false,
    )
    expect(isDraftsAbsent(err(500))).toBe(false)
  })
})

describe('hitlByStep', () => {
  const row = (id: string, idx: number | null, status = 'pending'): HitlDto =>
    ({
      id,
      kind: 'input_required',
      status,
      question: {},
      execution: { maf_step_index: idx },
    }) as unknown as HitlDto
  it('groups by the step that asked, in server order', () => {
    const g = hitlByStep(
      [step(0, 'success'), step(1, 'success')],
      [row('a', 1, 'resolved'), row('b', 0), row('c', 1)],
    )
    expect(g.get(0)!.map((r) => r.id)).toEqual(['b'])
    expect(g.get(1)!.map((r) => r.id)).toEqual(['a', 'c'])
  })
  it('an unindexed row goes to the paused step, else nowhere', () => {
    expect(
      hitlByStep([step(0, 'success'), step(1, 'awaiting_human')], [row('a', null)]).get(1),
    ).toHaveLength(1)
    expect(hitlByStep([step(0, 'success')], [row('a', null)]).size).toBe(0)
  })
})

const s = (text: string, agentId = '', agentName = ''): EditorStep => ({
  ...blankStep(),
  taskDescription: text,
  agentId,
  agentName,
})

describe('moveStep', () => {
  it('moves one step and leaves the rest in order', () => {
    expect(moveStep(['a', 'b', 'c'], 0, 2)).toEqual(['b', 'c', 'a'])
    expect(moveStep(['a', 'b', 'c'], 2, 1)).toEqual(['a', 'c', 'b'])
  })
  it('ignores a move off either end (same array back)', () => {
    const steps = ['a', 'b']
    expect(moveStep(steps, 0, -1)).toBe(steps)
    expect(moveStep(steps, 1, 2)).toBe(steps)
    expect(moveStep(steps, 1, 1)).toBe(steps)
  })
})

describe('toPayload', () => {
  it('trims, drops blank steps, omits an unset agent, numbers the sent steps, and maps indexes back', () => {
    const { steps, index } = toPayload([s('  '), s(' Research ', 'a-1'), s(''), s('Write')])
    // `step_index` is required by `main`'s PUT (W-2), counted over the steps sent.
    expect(steps).toEqual([
      { step_index: 0, task_description: 'Research', agent_id: 'a-1' },
      { step_index: 1, task_description: 'Write' },
    ])
    expect(index).toEqual([1, 3])
    // The server numbers the filtered list from 0; the cards from 1, blanks included.
    expect(remapStepError('step 1: task_description is required', index)).toBe(
      'Step 4: task_description is required',
    )
    expect(remapStepError('steps must not be empty', index)).toBe('steps must not be empty')
  })
})

describe('snapshot', () => {
  it('a blank step added or moved is a change; an agent name alone is not', () => {
    const base = [s('a', 'x', 'X'), s('b')]
    const clean = snapshot('n', '', base)
    expect(snapshot(' n ', '', base)).toBe(clean)
    expect(snapshot('n', '', [...base, s('')])).not.toBe(clean)
    expect(snapshot('n', '', moveStep(base, 0, 1))).not.toBe(clean)
    expect(snapshot('n', '', [{ ...base[0]!, agentName: 'renamed' }, base[1]!])).toBe(clean)
  })
})

describe('fromMaf / agentOptions', () => {
  it('seeds one blank step for a stepless definition', () => {
    expect(fromMaf(undefined)).toHaveLength(1)
    expect(fromMaf([])[0]!.taskDescription).toBe('')
  })
  it('labels agents by display name, flags the never-deployed, and leaves out coding harnesses', () => {
    const agents = [
      { id: '1', name: 'w', display_name: 'Writer', status: 'running', tags: [] },
      { id: '2', name: 'r', status: 'registered', tags: [] },
      { id: '3', name: 'claude', status: 'registered', tags: ['coding-agent'] },
    ] as unknown as Agent[]
    expect(agentOptions(agents)).toEqual([
      { id: '1', name: 'Writer', deployed: true },
      { id: '2', name: 'r', deployed: false },
    ])
  })
})
