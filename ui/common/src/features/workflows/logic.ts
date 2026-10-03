/**
 * Pure rules for workflows and their runs (plans/feat-workflows.md), ported from the React migration's
 * `domain/workflows/{model,editor}.ts` (themselves ports of legacy workflows-service.js, wf-run-steps.js and
 * wf-step-editor.js). Components stay thin; everything here is tested in logic.test.ts.
 */
import type { Agent } from '@/features/agents/types'
import { displayStatus, isHarness } from '@/features/agents/status'
import type { HitlDto } from '@/features/chat/types'
import { ApiError } from '@/lib/api/client'
import { isEndpointAbsent } from '@/lib/api/detect'
import type { RunAgeFilter, RunStatusFilter } from './search'
import type {
  Execution,
  ExecutionRow,
  GeneratedPlan,
  MafStep,
  StepInput,
  StepResult,
  Workflow,
  WorkflowRow,
} from './types'

/** A missing status counts as deployed (`main` has no drafts). */
export const isDeployed = (wf: Pick<Workflow, 'status'>) => (wf.status ?? 'active') !== 'draft'

export const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`

export const stepsOf = (wf: WorkflowRow) => wf.step_count ?? wf.maf_json?.steps?.length ?? 0
export const descriptionOf = (wf: Workflow) => wf.description || wf.maf_json?.description || ''
/** Distinct, in first-use order (NAS-697 `distinct_agent_names`; derived on `main`). */
export const agentsOf = (wf: WorkflowRow) =>
  (wf.agent_names ?? [...new Set((wf.maf_json?.steps ?? []).map((s) => s.agent_name))]).filter(
    Boolean,
  )
/** NAS-697 rows carry run metrics; `main`'s don't (W-3), so the page hides what it can't know. */
export const hasMetrics = (rows: WorkflowRow[]) => rows.some((r) => r.health !== undefined)

export type Tone = 'success' | 'warning' | 'info' | 'error' | 'neutral'

const EXEC_STATUS: Record<string, [string, Tone]> = {
  success: ['Complete', 'success'],
  failed: ['Failed', 'error'],
  // Blue, deliberately not green: running is not done.
  running: ['Running…', 'info'],
  awaiting_human: ['Awaiting action', 'warning'],
  stopped: ['Stopped', 'neutral'],
  pending: ['Queued', 'neutral'],
}
/** A run's badge: one vocabulary for the run view and the runs list. */
export const execStatus = (status: string) => {
  const [label, tone] = EXEC_STATUS[status] ?? [status, 'neutral' as const]
  return { label, tone }
}
/** Still moving: keep polling (a paused run included, or its answer is never seen). */
export const isExecActive = (status: string | undefined) =>
  status === 'pending' || status === 'running' || status === 'awaiting_human'

const STEP_STATUS: Record<string, [string, Tone]> = {
  success: ['Complete', 'success'],
  failed: ['Failed', 'error'],
  running: ['Running', 'info'],
  awaiting_human: ['Awaiting action', 'warning'],
  stopped: ['Stopped', 'neutral'],
}
/** A step's chip. A step that has not started is "Pending"; a run that has not is "Queued". */
export const stepStatus = (status: string) => {
  const [label, tone] = STEP_STATUS[status] ?? ['Pending', 'neutral' as const]
  return { label, tone }
}

/**
 * A card's status line, from the row's own last-run fields (legacy joined the newest 50 runs of all workflows, so
 * older ones read "Not run yet", and a stopped or paused run read "Running now"). Null on `main`: no fields to read.
 */
export function lastRun(
  row: WorkflowRow,
): { text: string; at?: string | null; tone: Tone | 'brand' } | null {
  if (!row.execution_count) return { text: 'Not run yet', tone: 'neutral' }
  if (!('last_run_status' in row)) return null
  const at = row.last_run_at
  switch (row.last_run_status) {
    case null:
    case undefined:
      // Runs exist but none has started.
      return { text: 'Queued', tone: 'neutral' }
    case 'success':
      return { text: 'Last run succeeded', at, tone: 'success' }
    case 'failed':
      return { text: 'Last run failed', at, tone: 'error' }
    case 'stopped':
      return { text: 'Last run stopped', at, tone: 'neutral' }
    case 'awaiting_human':
      return { text: 'Awaiting action', tone: 'warning' }
    default:
      return { text: 'Running now', tone: 'brand' }
  }
}

const STATUS_FILTER: Record<Exclude<RunStatusFilter, 'all'>, string[]> = {
  attention: ['awaiting_human'],
  running: ['pending', 'running'],
  success: ['success'],
  failed: ['failed', 'stopped'],
}
const AGE_DAYS: Record<Exclude<RunAgeFilter, 'any'>, number> = { '1d': 1, '7d': 7, '30d': 30 }

/** The runs list's card title; a gone workflow reads as such (and is searchable as such). */
export const runTitle = (r: ExecutionRow) =>
  `${r.workflow_name || 'Deleted workflow'} #${r.execution_number}`
export const isOrphan = (r: ExecutionRow) =>
  !r.maf_id || !r.workflow_name || r.workflow_status === 'deleted'

/** Client-side, over the fetched page only. */
export function filterRuns(
  rows: ExecutionRow[],
  f: { q: string; status: RunStatusFilter; age: RunAgeFilter },
  now: number,
) {
  const q = f.q.trim().toLowerCase()
  return rows.filter(
    (r) =>
      (f.status === 'all' || STATUS_FILTER[f.status].includes(r.status)) &&
      (f.age === 'any' || now - Date.parse(r.created_at) <= AGE_DAYS[f.age] * 86_400_000) &&
      (!q || runTitle(r).toLowerCase().includes(q)),
  )
}

/** "Step 2/5" while a run moves (capped: during the final synthesis every step is done), else "5 steps". */
export function runStepsLabel(e: Execution) {
  const steps = e.step_results ?? []
  if (!steps.length) return null
  if (!isExecActive(e.status)) return plural(steps.length, 'step')
  const done = steps.filter((s) => s.status === 'success').length
  return `Step ${Math.min(done + 1, steps.length)}/${steps.length}`
}

/** A retry is queued as `pending` with the failed attempt's error: not an error yet. */
export const showsRunError = (e: Execution) =>
  !!e.error && (e.status === 'failed' || e.status === 'stopped')

/** HITL rows per step index, server order. A row without an index belongs to the paused step; with none, dropped. */
export function hitlByStep(steps: StepResult[], hitl: HitlDto[] = []) {
  const paused = steps.find((s) => s.status === 'awaiting_human')?.step_index
  const out = new Map<number, HitlDto[]>()
  for (const row of hitl) {
    const i = row.execution?.maf_step_index ?? paused
    if (i == null) continue
    out.set(i, [...(out.get(i) ?? []), row])
  }
  return out
}

/** MAF's own tokens outside any step (planning, final synthesis), so the header total reconciles. */
export const planningTokens = (total: number, steps: StepResult[]) =>
  total - steps.reduce((n, s) => n + (s.tokens_used ?? 0), 0)

export type PlanFailure = 'no-key' | 'no-agents' | 'planner' | 'other'
/** Which of the planner's refusals this is (`generate_maf`: 503 no key, 400 no agents, 422 planning failed). */
export function planFailure(err: unknown): PlanFailure {
  const status = err instanceof ApiError ? err.status : 0
  return status === 503
    ? 'no-key'
    : status === 400
      ? 'no-agents'
      : status === 422
        ? 'planner'
        : 'other'
}

/**
 * The drafts routes are missing (W-1: `main`). There `/maf/workflow/drafts` matches `/maf/workflow/{id}` and fails
 * axum's UUID extractor with a plain-text 400, and `POST /maf/workflow/draft` is a 405. A proxy or an older build may
 * answer a bare 404. A MAF handler's own error is always the JSON envelope, so an envelope means the route exists.
 */
export function isDraftsAbsent(err: unknown): boolean {
  if (!(err instanceof ApiError)) return false
  const b = err.body
  if (b && typeof b === 'object' && 'status_code' in b) return false
  return (
    err.status === 405 ||
    (err.status === 400 && typeof b === 'string') ||
    (err.status === 404 && isEndpointAbsent(err))
  )
}

// ─── The step editor's model ────────────────────────────────────────────────

export interface EditorStep {
  /** Stable across edits and moves (a React key); never sent. */
  uid: string
  taskDescription: string
  /** '' = the server assigns one when the workflow is saved. */
  agentId: string
  agentName: string
  /** Proposed by the planner and not yet touched. */
  suggested: boolean
}

let seq = 0
const uid = () => `s${++seq}`

export const blankStep = (): EditorStep => ({
  uid: uid(),
  taskDescription: '',
  agentId: '',
  agentName: '',
  suggested: false,
})

/** A stored definition on screen; zero steps seeds one blank step. */
export const fromMaf = (steps: MafStep[] | undefined): EditorStep[] =>
  steps?.length
    ? steps.map((s) => ({
        uid: uid(),
        taskDescription: s.task_description,
        agentId: s.agent_id ?? '',
        agentName: s.agent_name ?? '',
        suggested: false,
      }))
    : [blankStep()]

export const fromPlan = (plan: GeneratedPlan): EditorStep[] =>
  plan.steps.map((s) => ({
    uid: uid(),
    taskDescription: s.task_description,
    agentId: s.agent_id ?? '',
    agentName: s.agent_name ?? '',
    suggested: true,
  }))

/** Moves one step; an out-of-range move is a no-op (same array). */
export function moveStep<T>(steps: T[], from: number, to: number): T[] {
  if (from === to || from < 0 || to < 0 || from >= steps.length || to >= steps.length) return steps
  const next = [...steps]
  next.splice(to, 0, ...next.splice(from, 1))
  return next
}

/**
 * The request's steps: trimmed, blank ones dropped, each with its `step_index` (required by `main`'s PUT, W-2).
 * `index[i]` is the editor position of request step `i`, because the server's "step {i}" errors count the sent list.
 */
export function toPayload(steps: EditorStep[]) {
  const out: StepInput[] = []
  const index: number[] = []
  steps.forEach((s, i) => {
    const task = s.taskDescription.trim()
    if (!task) return
    out.push({
      step_index: out.length,
      task_description: task,
      ...(s.agentId ? { agent_id: s.agentId } : {}),
    })
    index.push(i)
  })
  return { steps: out, index }
}

/** "step 0: …" from the server → "Step 2: …", numbered as the cards are. */
export const remapStepError = (message: string, index: number[]) =>
  message.replace(/^step (\d+)/, (m, i: string) => {
    const at = index[Number(i)]
    return at == null ? m : `Step ${at + 1}`
  })

/**
 * What "unsaved" compares: trimmed name, description and each step's text and agent. A blank step added or moved
 * counts; an agent's name alone does not.
 */
export const snapshot = (name: string, description: string, steps: EditorStep[]) =>
  JSON.stringify([
    name.trim(),
    description.trim(),
    steps.map((s) => [s.taskDescription.trim(), s.agentId || '']),
  ])

export interface AgentOption {
  id: string
  name: string
  /** Registered but never deployed: a run of it fails (legacy offered it silently). */
  deployed: boolean
}
/** The picker's agents, labelled as people know them; coding harnesses can't run a step. */
export const agentOptions = (agents: Agent[]): AgentOption[] =>
  agents
    .filter((a) => !isHarness(a))
    .map((a) => ({
      id: a.id,
      name: a.display_name || a.name || a.id,
      deployed: displayStatus(a.status, false) !== 'not-deployed',
    }))

/** The server's name for a bare-sentence draft (`derive_workflow_name`): the instruction's first 60 characters. */
export const derivedName = (instruction: string) => [...instruction].slice(0, 60).join('')
