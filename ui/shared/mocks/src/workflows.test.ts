/**
 * The workflows mock (plans/feat-workflows.md §8), asserted through the MSW handlers: NAS-697 list metrics and drafts,
 * runs that follow the clock, a pause answered through /api/hitl, and `?mock=workflows-classic` answering as `main`.
 */
import { afterEach, describe, expect, it } from 'vitest'
import { ApiError, apiData, apiFetch } from '@nasiko/data-access/client'
import { now, seed, setupPinnedSeed } from '@nasiko/testing/pinnedSeed'
import { configureMocks } from './handlers'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

interface Row {
  id: string
  name: string
  status: string
  success_rate?: number | null
  health?: string
  last_run_status?: string | null
  execution_count: number
  maf_json: { steps: { step_id: string }[] }
}
interface Run {
  id: string
  status: string
  step_results: { status: string }[]
  hitl?: { id: string; status: string }[]
  output: string | null
}
const json = (method: string, body?: unknown): RequestInit => ({
  method,
  headers: { 'Content-Type': 'application/json' },
  body: body === undefined ? undefined : JSON.stringify(body),
})
const list = async (path: string) => (await apiData<{ data: Row[] }>(path)).data
const run = (id: string) => apiData<Run>(`/api/maf/execution/${id}`)
const fail = (p: Promise<unknown>) =>
  p.then(
    () => {
      throw new Error('expected a failure')
    },
    (e: unknown) => e as ApiError,
  )

describe('workflows mock', () => {
  it('keeps the TokenOps seed workflows, with NAS-697 metrics from their runs', async () => {
    const rows = await list('/api/maf/workflows')
    const ticket = rows.find((r) => r.id === seed.workflows[0]!.maf_id)!
    expect(ticket.name).toBe(seed.workflows[0]!.workflow_name)
    // 11 of 12 succeeded, and the latest one failed.
    expect(ticket).toMatchObject({
      success_rate: 91.7,
      health: 'healthy',
      last_run_status: 'failed',
      execution_count: 12,
    })
    expect(rows.find((r) => r.name === 'Invoice triage')).toMatchObject({
      health: 'unknown',
      success_rate: null,
    })
    // Deleted workflows are gone from the list; drafts are their own list.
    expect(rows.map((r) => r.name)).not.toContain('Legacy export')
    expect(rows.every((r) => r.status === 'active')).toBe(true)
    const byHealth = await list('/api/maf/workflows?sort=health')
    expect(byHealth[0]!.health).toBe('degraded')
    expect(byHealth.at(-1)!.health).toBe('unknown')
  })

  it('saves, fills and promotes a draft, which stays in the drafts list once live', async () => {
    const draft = await apiData<Row>(
      '/api/maf/workflow/draft',
      json('POST', { instruction: 'Summarise every new support ticket' }),
    )
    expect(draft).toMatchObject({ status: 'draft', name: 'Summarise every new support ticket' })
    const e = await fail(apiData(`/api/maf/workflow/${draft.id}/promote`, json('POST')))
    expect(e.serverMessage).toMatch(/no steps to run/)
    await apiData(
      `/api/maf/workflow/${draft.id}`,
      json('PUT', { steps: [{ step_index: 0, task_description: 'Summarise the ticket' }] }),
    )
    const live = await apiData<Row>(`/api/maf/workflow/${draft.id}/promote`, json('POST'))
    expect(live.status).toBe('active')
    const drafts = await list('/api/maf/workflow/drafts')
    expect(drafts.find((r) => r.id === draft.id)?.status).toBe('active')
    expect((await list('/api/maf/workflows')).map((r) => r.id)).toContain(draft.id)
  })

  it('runs with the clock, pauses for approval, and resumes once answered', async () => {
    let t = now()
    configureMocks({ now: () => t })
    const contract = (await list('/api/maf/workflows')).find((r) => r.name === 'Contract intake')!
    const started = await apiData<{ execution_id: string }>(
      `/api/maf/workflow/${contract.id}/run`,
      json('POST'),
    )
    expect((await run(started.execution_id)).status).toBe('pending')
    t += 1_000
    expect((await run(started.execution_id)).step_results[0]!.status).toBe('running')
    t += 20_000
    const paused = await run(started.execution_id)
    expect(paused.status).toBe('awaiting_human')
    expect(paused.step_results.map((s) => s.status)).toEqual([
      'success',
      'success',
      'awaiting_human',
    ])
    const request = paused.hitl!.find((h) => h.status === 'pending')!
    await apiFetch(`/api/hitl/${request.id}/resolve`, json('POST', { answer: 'Approve' }))
    t += 20_000
    const done = await run(started.execution_id)
    expect(done.status).toBe('success')
    expect(done.output).toMatch(/Result of run/)
    expect(done.hitl![0]!.status).toBe('resolved')
  })

  it('stops the run when its request is dismissed', async () => {
    const runs = (await apiData<{ data: Run[] }>('/api/maf/executions')).data
    const waiting = runs.find((r) => r.status === 'awaiting_human')!
    await apiFetch(`/api/hitl/${waiting.hitl![0]!.id}/cancel`, json('POST'))
    expect((await run(waiting.id)).status).toBe('stopped')
  })

  it('stops a moving run where it is, and only once (feat/maf-stop-execution)', async () => {
    let t = now()
    configureMocks({ now: () => t })
    const contract = (await list('/api/maf/workflows')).find((r) => r.name === 'Contract intake')!
    const { execution_id: id } = await apiData<{ execution_id: string }>(
      `/api/maf/workflow/${contract.id}/run`,
      json('POST'),
    )
    t += 1_000
    const stopped = await apiData<{ previous_status: string }>(
      `/api/maf/execution/${id}/stop`,
      json('POST'),
    )
    expect(stopped.previous_status).toBe('running')
    t += 60_000
    const after = await run(id)
    expect(after.status).toBe('stopped')
    expect(after.step_results.map((s) => s.status)).toEqual(['stopped', 'stopped', 'stopped'])
    const again = await fail(apiFetch(`/api/maf/execution/${id}/stop`, json('POST')))
    expect(again.status).toBe(409)
    expect(again.serverMessage).toBe('execution already finished: stopped')
    const unknown = await fail(
      apiFetch('/api/maf/execution/00000000-0000-4000-8000-000000000001/stop', json('POST')),
    )
    expect(unknown.status).toBe(404)
  })

  it("withdraws a paused run's request when it is stopped", async () => {
    const runs = (await apiData<{ data: Run[] }>('/api/maf/executions')).data
    const waiting = runs.find((r) => r.status === 'awaiting_human')!
    await apiFetch(`/api/maf/execution/${waiting.id}/stop`, json('POST'))
    const after = await run(waiting.id)
    expect(after.status).toBe('stopped')
    expect(after.hitl![0]!.status).toBe('canceled')
    expect(after.step_results.map((s) => s.status)).toEqual(['success', 'success', 'stopped'])
  })

  it('stops every unfinished run of a workflow, and reports runs asked for by id', async () => {
    const contract = (await list('/api/maf/workflows')).find((r) => r.name === 'Contract intake')!
    await apiData(`/api/maf/workflow/${contract.id}/run`, json('POST'))
    const report = await apiData<{ stopped: { previous_status: string }[] }>(
      '/api/maf/executions/stop',
      json('POST', { workflow_id: contract.id }),
    )
    // The run just started (queued) and the seed's paused one; the filed run had finished.
    expect(report.stopped.map((r) => r.previous_status).sort()).toEqual([
      'awaiting_human',
      'pending',
    ])
    const runs = (await apiData<{ data: Run[] }>('/api/maf/executions')).data
    const done = runs.find((r) => r.status === 'success')!
    const unknown = '00000000-0000-4000-8000-000000000002'
    const byIds = await apiData<{
      stopped: unknown[]
      already_finished: { id: string; status: string }[]
      not_found: string[]
    }>('/api/maf/executions/stop', json('POST', { execution_ids: [done.id, unknown, done.id] }))
    expect(byIds).toMatchObject({
      stopped: [],
      already_finished: [{ id: done.id, status: 'success' }],
      not_found: [unknown],
    })
    for (const bad of [{}, { execution_ids: [done.id], workflow_id: contract.id }]) {
      const e = await fail(apiFetch('/api/maf/executions/stop', json('POST', bad)))
      expect(e.status).toBe(400)
      expect(e.serverMessage).toBeTruthy()
    }
    const malformed = await fail(
      apiFetch('/api/maf/executions/stop', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{not json',
      }),
    )
    expect(malformed.status).toBe(400)
    expect(malformed.serverMessage).toMatch(/^invalid request body/)
  })

  it('leaves stopped runs out of the success rate', async () => {
    // The second seed workflow (Quarterly report): 2 succeeded, 1 failed, 1 stopped.
    const report = (await list('/api/maf/workflows')).find(
      (r) => r.id === seed.workflows[1]!.maf_id,
    )
    expect(report?.success_rate).toBe(66.7)
  })

  it('answers as `main` under workflows-classic (W-1..W-4)', async () => {
    configureMocks({ variant: 'workflows-classic' })
    const rows = await list('/api/maf/workflows?sort=health')
    // No metrics, and the sort is ignored: newest first.
    expect(rows[0]).not.toHaveProperty('health')
    expect(rows.map((r) => r.name)[0]).toBe('Invoice triage')
    const drafts = await fail(apiData('/api/maf/workflow/drafts'))
    expect(drafts.status).toBe(400)
    expect(typeof drafts.body).toBe('string')
    expect(
      (await fail(apiData('/api/maf/workflow/draft', json('POST', { instruction: 'x' })))).status,
    ).toBe(405)
    // `step_index` is required on `main`'s PUT.
    const put = await fail(
      apiData(
        `/api/maf/workflow/${rows[0]!.id}`,
        json('PUT', { steps: [{ task_description: 'x' }] }),
      ),
    )
    expect(put.status).toBe(422)
    const runs = (await apiData<{ data: Run[] }>('/api/maf/executions')).data
    expect(runs[0]).not.toHaveProperty('hitl')
  })
})
