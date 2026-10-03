/**
 * The MAF workflows mock (plans/feat-workflows.md §8), shaped as nasiko-cloud-rs `main` `a6feda17`
 * `oss/server/src/maf.rs` plus the NAS-697 additions at nasiko-cloud-rs-react `65908711` (drafts, promote, sort, list
 * metrics, `hitl` on the runs list). `?mock=workflows-classic` answers as `main` alone:
 * - `/maf/workflow/drafts` hits `/maf/workflow/{id}` and fails axum's UUID extractor (a plain-text 400), a draft
 *   POST is a 405, promote a bare 404; list rows carry no metrics and `sort` is ignored; the runs list has no `hitl`;
 * - `UpdateStepRequest.step_index` is required (a missing one is axum's 422 JSON rejection).
 * Every other answer is the `{data, status_code, message}` envelope, errors too, with the handler's own messages.
 *
 * A run advances with the clock: queued for 0.5 s, then each step (`stepMs`), then the final synthesis. A step whose
 * task asks for approval pauses on an `input_required` request; answering it through `/api/hitl/:id/resolve` resumes the
 * run, dismissing it stops the run there (the mock's reading of the resume path; hitl ids this mock doesn't own fall
 * through to chat's handlers). Ids: workflows `5eed0010-*` (the three TokenOps seed workflows keep their `…f00n` ids),
 * runs `5eed0011-*`, requests `5eed0012-*`, steps `5eed0013-*`.
 */
import { http, HttpResponse, type HttpHandler } from 'msw'
import type { HitlDto } from '@/features/chat/types'
import type { AgentsState, MockAgent } from './agents'
import type { Seed } from './seed'

const QUEUE_MS = 500
const SYNTH_MS = 1_500
const PLANNING_TOKENS = 640
const SYNTH_TOKENS = 420
const DAY = 86_400_000
const APPROVAL = /\bapprov/i

interface MockStep {
  step_id: string
  step_index: number
  agent_id: string
  agent_name: string
  agent_endpoint: string
  task_description: string
}

interface MockWorkflow {
  id: string
  user_id: string
  name: string
  description: string | null
  steps: MockStep[]
  output_generation: string | null
  status: 'active' | 'draft' | 'deleted'
  /** NAS-697 `drafted_at`: the drafts list keeps promoted drafts too. */
  drafted: boolean
  created_at: string
  updated_at: string
}

type Outcome = { kind: 'success' } | { kind: 'fail' | 'stop'; at: number; error: string }

interface MockRun {
  id: string
  number: number
  maf_id: string
  user_id: string
  createdAt: number
  /** The definition as it was when the run started (`maf_executions.maf_json`). */
  steps: MockStep[]
  outcome: Outcome
  hitl: HitlDto[]
}

export interface WorkflowsState {
  workflows: MockWorkflow[]
  runs: MockRun[]
  nextWorkflow: number
  nextRun: number
  nextRequest: number
  nextStep: number
}

const pad = (n: number) => String(n).padStart(12, '0')
const iso = (ms: number) => new Date(ms).toISOString()
/** Deterministic 0..1 from two integers (tokens, durations). */
const vary = (a: number, b: number) => (((a * 2654435761) ^ (b * 40503)) >>> 0) / 2 ** 32
const stepMs = (run: MockRun, i: number) => 1_800 + Math.round(vary(run.number, i) * 2_400)
const stepTokens = (run: MockRun, i: number) => 700 + Math.round(vary(i + 7, run.number) * 1_600)

const deployed = (a: MockAgent) => a.status !== 'registered' && !a.deleted && a.harness === null

export function buildWorkflowsState(
  agents: AgentsState,
  seed: Seed,
  owner: string,
  now: number,
  opts: { empty?: boolean } = {},
): WorkflowsState {
  const st: WorkflowsState = {
    workflows: [],
    runs: [],
    nextWorkflow: 1,
    nextRun: 1,
    nextRequest: 1,
    nextStep: 1,
  }
  if (opts.empty) return st
  const agent = (key: string) => {
    const a = agents.agents.find((x) => x.name === `seed-${key}`)
    if (!a) throw new Error(`no seed agent ${key}`)
    return a
  }
  const add = (
    id: string | null,
    name: string,
    description: string | null,
    tasks: [string, string][],
    extra: Partial<MockWorkflow> & { ageDays: number },
  ) => {
    const wf: MockWorkflow = {
      id: id ?? `5eed0010-0000-4000-8000-${pad(st.nextWorkflow++)}`,
      user_id: owner,
      name,
      description,
      steps: tasks.map(([key, task], i) => mafStep(st, agent(key), task, i)),
      output_generation: tasks.length
        ? 'Lead with the outcome, then one short section per step. Keep it under 300 words.'
        : null,
      status: 'active',
      drafted: false,
      created_at: iso(now - extra.ageDays * DAY),
      updated_at: iso(now - Math.max(0, extra.ageDays - 1) * DAY),
      ...extra,
    }
    st.workflows.push(wf)
    return wf
  }
  const [f1, f2, f3] = seed.workflows
  const ticket = add(
    f1?.maf_id ?? null,
    f1?.workflow_name ?? 'Ticket resolution',
    'Classify a support ticket, look for known issues, and draft the reply.',
    [
      ['triage-router', "Classify the incoming ticket and pull the customer's plan and region"],
      ['research-agent', 'Find known issues and past tickets that match this one'],
      ['support-bot', 'Draft a reply that resolves the ticket, citing the known issue if any'],
    ],
    { ageDays: 30 },
  )
  const report = add(
    f2?.maf_id ?? null,
    f2?.workflow_name ?? 'Quarterly report',
    'Revenue and churn by region, the biggest changes, and a one-page brief.',
    [
      ['sql-analyst', 'Pull revenue and churn by region for the quarter'],
      ['growth-analyst', 'Explain the three biggest changes against last quarter'],
      ['sales-assistant', 'Write the one-page brief for the leadership review'],
    ],
    { ageDays: 24 },
  )
  const contract = add(
    f3?.maf_id ?? null,
    f3?.workflow_name ?? 'Contract intake',
    'Read a new contract, flag risky clauses, and file it once approved.',
    [
      ['research-agent', 'Extract the parties, term, renewal date and payment terms'],
      ['legal-reviewer', 'Flag the clauses that need legal review'],
      ['legal-reviewer', 'Ask for approval before filing the contract'],
    ],
    { ageDays: 18 },
  )
  add(
    null,
    'Invoice triage',
    'Clean up invoice lines and match each one to its purchase order.',
    [
      ['data-cleaner', 'Normalise the invoice line items'],
      ['sql-analyst', 'Match each invoice to its purchase order'],
    ],
    { ageDays: 3 },
  )
  add(
    null,
    'Onboarding checklist',
    "Build a new hire's first-week checklist in their language.",
    [
      ['onboarding-guide', "Build the new hire's first-week checklist"],
      ['translator', "Translate the checklist into the hire's language"],
    ],
    { ageDays: 2, status: 'draft', drafted: true, updated_at: iso(now - 2 * 3_600_000) },
  )
  const churn = 'Warn the account team when a customer shows early signs of churn'
  add(null, [...churn].slice(0, 60).join(''), churn, [], {
    ageDays: 1,
    status: 'draft',
    drafted: true,
  })
  const gone = add(
    null,
    'Legacy export',
    'Exported the old CRM once a week.',
    [['data-cleaner', 'Export the CRM accounts to CSV']],
    {
      ageDays: 40,
      status: 'deleted',
    },
  )

  const run = (wf: MockWorkflow, ageMs: number, outcome: Outcome = { kind: 'success' }) =>
    startRun(st, wf, now - ageMs, outcome)
  // Ticket resolution: 11 of 12 succeed (91.7%, healthy) and the latest failed.
  for (let i = 12; i >= 2; i--) run(ticket, i * DAY + 3_600_000)
  run(ticket, 5 * 3_600_000, {
    kind: 'fail',
    at: 1,
    error: 'agent research-agent returned 502 Bad Gateway: upstream timed out after 60s',
  })
  // Quarterly report: 2 of 4 (degraded), one failed, one stopped.
  run(report, 20 * DAY)
  run(report, 13 * DAY, {
    kind: 'fail',
    at: 0,
    error: 'agent sql-analyst returned 500: relation "revenue_q3" does not exist',
  })
  run(report, 6 * DAY)
  run(report, 2 * DAY, { kind: 'stop', at: 2, error: 'The run was stopped before it finished.' })
  // Contract intake: one approved and filed, one waiting for approval now.
  const filed = run(contract, 9 * DAY)
  answer(st, filed, 2, 'Approve', 4 * 60_000, now)
  run(contract, 6 * 60_000)
  run(gone, 35 * DAY)
  return st
}

function mafStep(st: WorkflowsState, a: MockAgent, task: string, i: number): MockStep {
  return {
    step_id: `5eed0013-0000-4000-8000-${pad(st.nextStep++)}`,
    step_index: i,
    agent_id: a.id,
    agent_name: a.name,
    agent_endpoint: `http://localhost:${9100 + (Number(a.id.slice(-2)) || 0)}`,
    task_description: task,
  }
}

function startRun(st: WorkflowsState, wf: MockWorkflow, at: number, outcome: Outcome): MockRun {
  const r: MockRun = {
    id: `5eed0011-0000-4000-8000-${pad(st.nextRun)}`,
    number: st.nextRun++,
    maf_id: wf.id,
    user_id: wf.user_id,
    createdAt: at,
    steps: wf.steps.map((s) => ({ ...s })),
    outcome,
    hitl: [],
  }
  st.runs.push(r)
  return r
}

/** The approval request a step raises when the run reaches it. */
function request(st: WorkflowsState, run: MockRun, i: number, at: number): HitlDto {
  const step = run.steps[i]!
  const h: HitlDto = {
    id: `5eed0012-0000-4000-8000-${pad(st.nextRequest++)}`,
    kind: 'input_required',
    status: 'pending',
    resume_status: 'not_started',
    question: {
      message: `${step.task_description}: approve this step?`,
      options: [
        { label: 'Approve', description: 'Continue the run' },
        { label: 'Send back', description: 'Continue, noting that it needs changes' },
      ],
      allow_custom_input: true,
    } as HitlDto['question'],
    human_response: null,
    execution: {
      origin: 'maf',
      agent_id: step.agent_id,
      task_id: null,
      context_id: null,
      chat_session_id: null,
      maf_execution_id: run.id,
      maf_step_index: i,
    },
    allowed_actions: ['answer', 'cancel'],
    expires_at: iso(at + 7 * DAY),
    created_at: iso(at),
    resolved_at: null,
  }
  run.hitl.push(h)
  return h
}

/** Seeds an answered request: deriving the run reaches the pause (raising it), then it is answered. */
function answer(
  st: WorkflowsState,
  run: MockRun,
  i: number,
  label: string,
  afterMs: number,
  now: number,
) {
  deriveRun(run, now, st)
  const h = run.hitl.find((x) => x.execution.maf_step_index === i)
  if (!h) return
  h.status = 'resolved'
  h.resume_status = 'completed'
  h.human_response = { answer: label }
  h.resolved_at = iso(Date.parse(h.created_at) + afterMs)
}

interface StepView {
  step_id: string
  step_index: number
  agent_id: string
  agent_name: string
  status: string
  error: string | null
  prompt: string | null
  extracted_info: string | null
  tokens_used: number
  latency_ms: number
}

/** A run as the server would report it at `now`. `st` creates a pause's request when the run first reaches it. */
function deriveRun(run: MockRun, now: number, st: WorkflowsState | null) {
  const steps: StepView[] = run.steps.map((s) => ({
    step_id: s.step_id,
    step_index: s.step_index,
    agent_id: s.agent_id,
    agent_name: s.agent_name,
    status: 'pending',
    error: null,
    prompt: null,
    extracted_info: null,
    tokens_used: 0,
    latency_ms: 0,
  }))
  const base = {
    id: run.id,
    execution_number: run.number,
    maf_id: run.maf_id,
    user_id: run.user_id,
    attempt_count: 0,
    max_attempts: 3,
    created_at: iso(run.createdAt),
  }
  let t = run.createdAt + QUEUE_MS
  if (now < t)
    return {
      ...base,
      status: 'pending',
      tokens_used: 0,
      started_at: null,
      completed_at: null,
      duration_ms: null,
      output: null,
      step_results: steps,
      error: null,
    }
  const started = t
  let tokens = PLANNING_TOKENS
  const done = (status: string, end: number, extra: { output?: string; error?: string }) => ({
    ...base,
    attempt_count: 1,
    status,
    tokens_used: tokens,
    started_at: iso(started),
    completed_at: iso(end),
    duration_ms: end - started,
    output: extra.output ?? null,
    step_results: steps,
    error: extra.error ?? null,
  })
  const moving = (status: string) => ({
    ...base,
    attempt_count: 1,
    status,
    tokens_used: tokens,
    started_at: iso(started),
    completed_at: null,
    duration_ms: null,
    output: null,
    step_results: steps,
    error: null,
  })
  const answers: string[] = []
  for (let i = 0; i < run.steps.length; i++) {
    const step = run.steps[i]!
    const view = steps[i]!
    const prompt = `${step.task_description}\n\nContext from the previous steps:\n${
      answers.length ? answers.map((a) => `- ${a}`).join('\n') : '- (none: this is the first step)'
    }`
    if (APPROVAL.test(step.task_description)) {
      let h = run.hitl.find((x) => x.execution.maf_step_index === i)
      if (!h && st) h = request(st, run, i, t)
      if (!h || h.status === 'pending') {
        view.status = 'awaiting_human'
        view.prompt = h?.question?.message ?? step.task_description
        return moving('awaiting_human')
      }
      if (h.status !== 'resolved') {
        view.status = 'stopped'
        view.error = 'The approval request was dismissed, so the run stopped here.'
        return done('stopped', Date.parse(h.resolved_at ?? iso(t)), {
          error: 'The approval request was dismissed.',
        })
      }
      t = Math.max(t, Date.parse(h.resolved_at ?? iso(t)))
    }
    const end = t + stepMs(run, i)
    if (now < end) {
      view.status = 'running'
      return moving('running')
    }
    view.prompt = prompt
    view.latency_ms = end - t
    const o = run.outcome
    if (o.kind !== 'success' && o.at === i) {
      view.status = o.kind === 'fail' ? 'failed' : 'stopped'
      view.error = o.error
      return done(view.status, end, { error: o.error })
    }
    view.status = 'success'
    view.tokens_used = stepTokens(run, i)
    tokens += view.tokens_used
    view.extracted_info = extracted(step, i, run)
    answers.push(`${step.agent_name}: ${view.extracted_info.split('\n')[0]}`)
    t = end
  }
  if (now < t + SYNTH_MS) return moving('running')
  tokens += SYNTH_TOKENS
  return done('success', t + SYNTH_MS, { output: output(run, steps) })
}

function extracted(step: MockStep, i: number, run: MockRun): string {
  const approved = run.hitl.find((x) => x.execution.maf_step_index === i)
  if (approved) {
    const a = (approved.human_response as { answer?: unknown } | null)?.answer
    return `**${String(a ?? 'Approved')}** — recorded by the reviewer.\n\nThe contract is filed under Legal › Intake.`
  }
  return `Done: ${step.task_description.toLowerCase()}.\n\n- Checked ${3 + (run.number % 5)} sources\n- Confidence: ${
    80 + (run.number % 17)
  }%`
}

function output(run: MockRun, steps: StepView[]): string {
  return [
    `## Result of run #${run.number}`,
    '',
    ...steps.map((s, i) => `${i + 1}. **${s.agent_name}** — ${run.steps[i]!.task_description}.`),
    '',
    'Every step finished. The summary above follows the output guidelines.',
  ].join('\n')
}

// ─── Handlers ──────────────────────────────────────────────────────────────

export interface WorkflowsCtx {
  loggedIn: () => boolean
  me: () => { id: string; is_superuser: boolean }
  agents: () => AgentsState
  /** Agent ids the caller can see (owner, public or granted), as `acl::can_access_agent`. */
  canAccess: (agentId: string) => boolean
  now: () => number
  hasVariant: (v: string) => boolean
  state: () => WorkflowsState
}

class MafError extends Error {
  readonly status: number
  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
}
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
/** axum's `Path<Uuid>` rejection: plain text, before the handler runs. */
const badPath = (seg: string) =>
  new HttpResponse(
    `Invalid URL: UUID parsing failed: invalid character: expected an optional prefix of \`urn:uuid:\` followed by [0-9a-fA-F-], found \`${seg[1] ?? seg[0] ?? ''}\` at 2`,
    { status: 400, headers: { 'Content-Type': 'text/plain; charset=utf-8' } },
  )
const ok = (data: unknown, message: string, status = 200) =>
  HttpResponse.json({ data, status_code: status, message }, { status })

export function workflowHandlers(ctx: WorkflowsCtx): HttpHandler[] {
  const classic = () => ctx.hasVariant('workflows-classic')
  const guard = async (fn: () => Response | Promise<Response>): Promise<Response> => {
    if (!ctx.loggedIn())
      return HttpResponse.json(
        { data: null, status_code: 401, message: 'invalid or missing user identity' },
        { status: 401 },
      )
    try {
      return await fn()
    } catch (err) {
      if (err instanceof MafError)
        return HttpResponse.json(
          { data: null, status_code: err.status, message: err.message },
          { status: err.status },
        )
      throw err
    }
  }
  const s = () => ctx.state()
  const now = () => ctx.now()
  /** `fetch_maf`: active rows (NAS-697: drafts too), then the owner check. */
  const owned = (id: string) => {
    const wf = s().workflows.find(
      (w) => w.id === id && (w.status === 'active' || (!classic() && w.status === 'draft')),
    )
    if (!wf) throw new MafError(404, 'workflow not found')
    if (wf.user_id !== ctx.me().id) throw new MafError(403, 'not owned by caller')
    return wf
  }
  const mafJson = (wf: MockWorkflow) => ({
    description: null,
    steps: wf.steps,
    output_generation: wf.output_generation,
  })
  const runsOf = (wf: MockWorkflow) => s().runs.filter((r) => r.maf_id === wf.id)
  const response = (wf: MockWorkflow) => ({
    id: wf.id,
    user_id: wf.user_id,
    name: wf.name,
    description: wf.description,
    maf_json: mafJson(wf),
    status: wf.status,
    created_at: wf.created_at,
    updated_at: wf.updated_at,
    execution_count: runsOf(wf).length,
  })
  /** NAS-697 `WorkflowListResponse`: the metrics come from the runs as they stand now. */
  const listRow = (wf: MockWorkflow) => {
    if (classic()) return response(wf)
    const views = runsOf(wf).map((r) => deriveRun(r, now(), s()))
    const rate = views.length
      ? Math.round((1000 * views.filter((v) => v.status === 'success').length) / views.length) / 10
      : null
    const startedViews = views.filter((v) => v.started_at)
    const latest = startedViews.sort((a, b) => b.started_at!.localeCompare(a.started_at!))[0]
    return {
      ...response(wf),
      success_rate: rate,
      health:
        rate === null ? 'unknown' : rate >= 90 ? 'healthy' : rate >= 50 ? 'degraded' : 'unhealthy',
      total_tokens: views.reduce((n, v) => n + v.tokens_used, 0),
      last_run_at: latest?.started_at ?? null,
      last_run_status: latest?.status ?? null,
      step_count: wf.steps.length,
      agent_names: [...new Map(wf.steps.map((x) => [x.agent_id, x.agent_name])).values()],
    }
  }
  type Row = ReturnType<typeof listRow> & {
    success_rate?: number | null
    total_tokens?: number
  }
  const ORDER: Record<string, (a: Row, b: Row) => number> = {
    success_rate: (a, b) => nullsLast(b.success_rate, a.success_rate),
    health: (a, b) => nullsLast(a.success_rate, b.success_rate),
    token_usage: (a, b) => (b.total_tokens ?? 0) - (a.total_tokens ?? 0),
    execution_count: (a, b) => b.execution_count - a.execution_count,
    last_updated: (a, b) => b.updated_at.localeCompare(a.updated_at),
  }
  const page = (list: MockWorkflow[], url: URL) => {
    const rows: Row[] = list.map(listRow).sort((a, b) => b.created_at.localeCompare(a.created_at))
    const by = classic() ? undefined : ORDER[url.searchParams.get('sort') ?? '']
    if (by) rows.sort((a, b) => by(a, b) || b.created_at.localeCompare(a.created_at))
    const limit = Number(url.searchParams.get('limit') ?? 50)
    const offset = Number(url.searchParams.get('offset') ?? 0)
    const data = rows.slice(offset, offset + limit)
    return { data, total: data.length }
  }

  /** `create_maf` / `update_maf` step resolution: an explicit agent must be reachable, else route by words. */
  const resolve = (steps: unknown, update: boolean): MockStep[] => {
    if (!Array.isArray(steps) || !steps.length) throw new MafError(400, 'steps must not be empty')
    const mine = ctx
      .agents()
      .agents.filter((a) => a.owner_id === ctx.me().id && deployed(a) && a.status === 'running')
    return steps.map((raw: unknown, idx) => {
      const step = (raw ?? {}) as {
        task_description?: unknown
        agent_id?: unknown
        step_index?: unknown
      }
      const task = typeof step.task_description === 'string' ? step.task_description : ''
      if (!task.trim())
        throw new MafError(
          400,
          `step ${update && typeof step.step_index === 'number' ? step.step_index : idx}: task_description is required`,
        )
      let a: MockAgent | undefined
      if (typeof step.agent_id === 'string' && step.agent_id) {
        a = ctx.agents().agents.find((x) => x.id === step.agent_id && !x.deleted)
        if (!a || !ctx.canAccess(a.id)) throw new MafError(403, `agent ${step.agent_id} not found`)
      } else {
        const words = task.toLowerCase().split(/\s+/)
        a = [...mine]
          .map((x) => ({
            x,
            score: words.filter((w) => `${x.name} ${x.description}`.toLowerCase().includes(w))
              .length,
          }))
          .sort((p, q) => q.score - p.score)[0]?.x
        if (!a)
          throw new MafError(
            400,
            update
              ? `step ${idx}: no agents available. Register at least one agent in the Agents page.`
              : `step ${idx}: no deployed agent is available to run this step. Deploy at least one agent (a registered agent with no running container has no endpoint to call) before creating a workflow.`,
          )
      }
      return mafStep(s(), a, task, idx)
    })
  }
  const body = async (request: Request) =>
    (await request.json().catch(() => ({}))) as Record<string, unknown>
  const text = (v: unknown) => (typeof v === 'string' ? v.trim() : '')

  const mafHitl = (id: string) => {
    for (const r of s().runs) {
      const h = r.hitl.find((x) => x.id === id)
      if (h) return { run: r, h }
    }
    return null
  }

  return [
    http.get('/api/maf/workflows', ({ request }) =>
      guard(() => {
        const mine = s().workflows.filter((w) => w.user_id === ctx.me().id && w.status === 'active')
        return ok(page(mine, new URL(request.url)), 'Workflows retrieved successfully')
      }),
    ),
    http.post('/api/maf/workflows', ({ request }) =>
      guard(async () => {
        const b = await body(request)
        const steps = resolve(b.steps, false)
        const at = iso(now())
        const wf: MockWorkflow = {
          id: `5eed0010-0000-4000-8000-${pad(s().nextWorkflow++)}`,
          user_id: ctx.me().id,
          name: text(b.name) || [...steps[0]!.task_description].slice(0, 60).join(''),
          description: text(b.description) || null,
          steps,
          output_generation: null,
          status: 'active',
          drafted: false,
          created_at: at,
          updated_at: at,
        }
        s().workflows.push(wf)
        return ok(response(wf), 'Workflow created successfully', 201)
      }),
    ),
    http.get('/api/maf/workflow/drafts', ({ request }) =>
      guard(() => {
        if (classic()) return badPath('drafts')
        const mine = s().workflows.filter(
          (w) => w.user_id === ctx.me().id && w.drafted && w.status !== 'deleted',
        )
        return ok(page(mine, new URL(request.url)), 'Drafts retrieved successfully')
      }),
    ),
    http.post('/api/maf/workflow/draft', ({ request }) =>
      guard(async () => {
        if (classic()) return new HttpResponse(null, { status: 405 })
        const b = await body(request)
        const instruction = text(b.instruction)
        if (!instruction) throw new MafError(400, 'instruction is required')
        const name = [...instruction].slice(0, 60).join('')
        const at = iso(now())
        if (typeof b.draft_id === 'string') {
          const wf = s().workflows.find(
            (w) => w.id === b.draft_id && w.user_id === ctx.me().id && w.status === 'draft',
          )
          if (!wf) throw new MafError(404, 'draft not found')
          Object.assign(wf, { name, description: instruction, updated_at: at })
          return ok(response(wf), 'Draft saved')
        }
        const wf: MockWorkflow = {
          id: `5eed0010-0000-4000-8000-${pad(s().nextWorkflow++)}`,
          user_id: ctx.me().id,
          name,
          description: instruction,
          steps: [],
          output_generation: null,
          status: 'draft',
          drafted: true,
          created_at: at,
          updated_at: at,
        }
        s().workflows.push(wf)
        return ok(response(wf), 'Draft saved')
      }),
    ),
    http.post('/api/maf/workflow/:id/promote', ({ params }) =>
      guard(() => {
        if (classic()) return new HttpResponse(null, { status: 404 })
        const id = String(params.id)
        if (!UUID.test(id)) return badPath(id)
        const wf = s().workflows.find((w) => w.id === id && w.status !== 'deleted')
        if (!wf) throw new MafError(404, 'draft not found')
        if (wf.user_id !== ctx.me().id) throw new MafError(403, 'not owned by caller')
        if (wf.status !== 'draft')
          throw new MafError(
            400,
            `workflow is already '${wf.status}' — only a draft can be promoted`,
          )
        if (!wf.steps.length)
          throw new MafError(
            400,
            'this draft has no steps to run — create it with POST /api/maf/workflow/from-instruction, which decomposes the instruction and assigns an agent to each step',
          )
        wf.status = 'active'
        wf.updated_at = iso(now())
        return ok(response(wf), 'Draft promoted')
      }),
    ),
    http.get('/api/maf/workflow/:id', ({ params }) =>
      guard(() => {
        const id = String(params.id)
        if (!UUID.test(id)) return badPath(id)
        return ok(response(owned(id)), 'Workflow retrieved successfully')
      }),
    ),
    http.put('/api/maf/workflow/:id', ({ params, request }) =>
      guard(async () => {
        const id = String(params.id)
        if (!UUID.test(id)) return badPath(id)
        const b = await body(request)
        const wf = owned(id)
        if (classic() && Array.isArray(b.steps)) {
          const i = b.steps.findIndex(
            (x: unknown) => typeof (x as { step_index?: unknown }).step_index !== 'number',
          )
          if (i >= 0)
            return new HttpResponse(
              `Failed to deserialize the JSON body into the target type: steps[${i}]: missing field \`step_index\``,
              { status: 422, headers: { 'Content-Type': 'text/plain; charset=utf-8' } },
            )
        }
        if (b.steps !== undefined) wf.steps = resolve(b.steps, true)
        if (typeof b.name === 'string') wf.name = b.name.trim()
        if ('description' in b)
          wf.description = typeof b.description === 'string' ? b.description : null
        wf.updated_at = iso(now())
        return ok(response(wf), 'Workflow updated successfully')
      }),
    ),
    http.delete('/api/maf/workflow/:id', ({ params }) =>
      guard(() => {
        const id = String(params.id)
        if (!UUID.test(id)) return badPath(id)
        const wf = s().workflows.find((w) => w.id === id && w.status !== 'deleted')
        if (!wf) throw new MafError(404, 'workflow not found')
        if (wf.user_id !== ctx.me().id && !ctx.me().is_superuser)
          throw new MafError(403, 'not owned by caller')
        wf.status = 'deleted'
        wf.updated_at = iso(now())
        return ok(null, 'Workflow deleted successfully')
      }),
    ),
    http.post('/api/maf/workflow/:id/run', ({ params }) =>
      guard(() => {
        const id = String(params.id)
        if (!UUID.test(id)) return badPath(id)
        const wf = owned(id)
        const r = startRun(s(), wf, now(), { kind: 'success' })
        return ok(
          { execution_id: r.id, execution_number: r.number, execution_count: runsOf(wf).length },
          'Execution started successfully',
          202,
        )
      }),
    ),
    http.get('/api/maf/executions', ({ request }) =>
      guard(() => {
        const url = new URL(request.url)
        const limit = Math.min(Number(url.searchParams.get('limit') ?? 50), 50)
        const offset = Number(url.searchParams.get('offset') ?? 0)
        const data = s()
          .runs.filter((r) => r.user_id === ctx.me().id)
          .sort((a, b) => b.createdAt - a.createdAt)
          .slice(offset, offset + limit)
          .map((r) => {
            const wf = s().workflows.find((w) => w.id === r.maf_id)
            return {
              ...deriveRun(r, now(), s()),
              workflow_name: wf?.name ?? null,
              workflow_status: wf?.status ?? null,
              ...(classic() ? {} : { hitl: r.hitl }),
            }
          })
        return ok({ data, total: data.length }, 'Executions retrieved successfully')
      }),
    ),
    http.get('/api/maf/execution/:id', ({ params }) =>
      guard(() => {
        const id = String(params.id)
        if (!UUID.test(id)) return badPath(id)
        const r = s().runs.find((x) => x.id === id)
        if (!r) throw new MafError(404, 'execution not found')
        if (r.user_id !== ctx.me().id) throw new MafError(403, 'not owned by caller')
        return ok({ ...deriveRun(r, now(), s()), hitl: r.hitl }, 'Execution retrieved successfully')
      }),
    ),
    http.post('/api/maf/generate', ({ request }) =>
      guard(async () => {
        const description = text((await body(request)).description)
        if (!description) throw new MafError(400, 'description is required')
        if (ctx.hasVariant('workflows-no-key'))
          throw new MafError(503, 'OPENAI_API_KEY is not configured on this server')
        const mine = ctx
          .agents()
          .agents.filter((a) => a.owner_id === ctx.me().id && !a.deleted && a.harness === null)
        if (!mine.length)
          throw new MafError(
            400,
            'no agents registered — register at least one agent before generating a MAF',
          )
        if (ctx.hasVariant('workflows-planner-fails'))
          throw new MafError(422, 'planning failed: the model returned no steps')
        return ok(plan(description, mine), 'Workflow plan generated successfully')
      }),
    ),

    // A step's request: this mock's ids only; anything else falls through to chat's handlers.
    http.get('/api/hitl/:id', ({ params }) => {
      const hit = mafHitl(String(params.id))
      if (!hit) return undefined
      return guard(() => HttpResponse.json(hit.h))
    }),
    http.post('/api/hitl/:id/resolve', async ({ params, request }) => {
      const hit = mafHitl(String(params.id))
      if (!hit) return undefined
      return guard(async () => {
        const { h } = hit
        if (h.status === 'expired' || h.status === 'canceled')
          return new HttpResponse(`this HITL request was ${h.status} before it was answered`, {
            status: 409,
          })
        if (h.status !== 'pending') return HttpResponse.json({ ...h, already_resolved: true })
        const b = (await body(request)) as { answer?: unknown; custom_answer?: unknown }
        const answer = Array.isArray(b.answer) ? b.answer.map(String) : text(b.answer)
        const custom = text(b.custom_answer)
        if (!answer.length && !custom)
          return new HttpResponse('answer is required for input_required', { status: 400 })
        h.status = 'resolved'
        h.resume_status = 'completed'
        h.human_response = {
          answer: answer.length ? answer : custom,
          ...(custom ? { custom_answer: custom } : {}),
        }
        h.resolved_at = iso(now())
        return HttpResponse.json({ ...h, already_resolved: false })
      })
    }),
    http.post('/api/hitl/:id/cancel', ({ params }) => {
      const hit = mafHitl(String(params.id))
      if (!hit) return undefined
      return guard(() => {
        const { h } = hit
        if (h.status === 'canceled') return HttpResponse.json({ ...h, already_canceled: true })
        if (h.status !== 'pending')
          return new HttpResponse(`this HITL request is already ${h.status}, not pending`, {
            status: 409,
          })
        h.status = 'canceled'
        h.resolved_at = iso(now())
        return HttpResponse.json(h)
      })
    }),
  ]
}

const nullsLast = (a: number | null | undefined, b: number | null | undefined) =>
  a == null ? (b == null ? 0 : 1) : b == null ? -1 : a - b

/** The planner's proposal: a clause per step, each routed to the best-matching agent by words. */
function plan(description: string, agents: MockAgent[]) {
  const clauses = description
    .split(/(?:,|;|\.|\bthen\b|\band then\b)\s*/i)
    .map((c) => c.trim())
    .filter((c) => c.length > 3)
    .slice(0, 4)
  const tasks = clauses.length ? clauses : [description]
  const steps = tasks.map((task) => {
    const words = task.toLowerCase().split(/\s+/)
    const a =
      [...agents]
        .map((x) => ({
          x,
          score: words.filter((w) =>
            `${x.name} ${x.display_name} ${x.description}`.toLowerCase().includes(w),
          ).length,
        }))
        .sort((p, q) => q.score - p.score)[0]?.x ?? agents[0]!
    return {
      agent_id: a.id,
      agent_name: a.name,
      task_description: task[0]!.toUpperCase() + task.slice(1),
    }
  })
  const name = description.split(/\s+/).slice(0, 5).join(' ')
  return {
    name: name[0]!.toUpperCase() + name.slice(1),
    description,
    output_generation: 'Lead with the outcome, then one short section per step.',
    steps,
  }
}

/** For tests: the runs as the server would report them now. */
export const viewRun = (st: WorkflowsState, id: string, now: number) => {
  const r = st.runs.find((x) => x.id === id)
  return r ? deriveRun(r, now, st) : null
}
