/**
 * The workflows mock (plans/feat-workflows.md §8), asserted through the MSW handlers: NAS-697 list metrics and drafts,
 * runs that follow the clock, a pause answered through /api/hitl, and `?mock=workflows-classic` answering as `main`.
 */
import { afterEach, describe, expect, it } from 'vitest'
import { ApiError, apiData, apiFetch } from '@/lib/api/client'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
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
