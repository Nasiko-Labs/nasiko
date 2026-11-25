/**
 * The evals routes (`/api/evals/*`, nasiko-cloud-rs `oss/server/src/evals/api.rs`; docs/superpowers/specs/2026-11-23-evals-design.md),
 * mocked under the `evals` MOCKABLE group. `?mock=evals-absent` answers the bare 404 a server without them gives.
 *
 * Harnesses and sessions come from the harness seed (the viewer's live harness agents and their sessions), so Evals
 * agrees with Coding harnesses, and `no-harnesses` empties both. Tasks, scores, costs and latencies are derived from a
 * hash of the ids, so the same picks always give the same table. Like the server, a dataset is built and a run is
 * replayed in the background: each reads as in progress until it is first read again, then as finished.
 * `?mock=evals-github-off` answers as for a viewer whose GitHub is not connected. `no-activity` leaves Claude Code
 * connected with no sessions, which is when the page offers the benchmarks (`GET /api/evals/benchmarks`, which no server
 * has yet: the page reads its absence as none).
 */
import { http, HttpResponse, type HttpHandler } from 'msw'
import type {
  EvalBenchmark,
  EvalDataset,
  EvalModel,
  EvalResult,
  EvalRun,
  EvalSession,
  EvalTask,
} from '@nasiko/features/evals/types'
import { type HarnessSeed, type HSession } from './seed-harness'

export interface EvalsCtx {
  loggedIn: () => boolean
  unauthorized: () => Response
  /** `?mock=evals-absent`. */
  absent: () => boolean
  /** `?mock=evals-github-off`. */
  githubOff: () => boolean
  harnessSeed: () => HarnessSeed
  viewerId: () => string
}

/** ponytail: the newest 50 sessions only; page with a cursor once a harness has more than a screenful worth picking. */
const SESSION_CAP = 50

const PROMPTS = [
  'Add retries with backoff to the billing webhook handler',
  'Fix the flaky date test in the invoices suite',
  'Refactor the auth middleware into smaller functions',
  'Write a migration that adds an index on sessions.user_id',
  'Explain why the build cache misses on CI',
  'Add pagination to the agents list endpoint',
  'Port the CSV export from Python to TypeScript',
  'Find and fix the memory leak in the SSE reader',
  'Add input validation to the connector registration form',
  'Write unit tests for the rate limiter',
  'Rename the config keys to the new naming scheme',
  'Speed up the dashboard query that times out on large orgs',
]

/** Public SWE benchmarks by name, with a few sample tasks each: the real lists come from the server. */
const BENCHMARKS: (Omit<EvalBenchmark, 'tasks'> & { tasks: string[] })[] = [
  {
    id: 'swe-bench-verified',
    name: 'SWE-bench Verified',
    description:
      'Real GitHub issues from popular Python repositories, each checked by people to be solvable.',
    tasks: [
      'astropy-12907: separability_matrix is wrong for nested compound models',
      'django-11099: UsernameValidator accepts a trailing newline',
      'sympy-20590: Symbol instances have a __dict__ again',
      'scikit-learn-13142: GaussianMixture fit_predict and predict disagree when n_init > 1',
      'matplotlib-23314: set_visible() has no effect on 3D axes',
      'requests-2317: a bytes HTTP method turns into the string "b\'GET\'"',
    ],
  },
  {
    id: 'swe-bench-lite',
    name: 'SWE-bench Lite',
    description: 'A smaller, cheaper slice of SWE-bench: self-contained bug fixes.',
    tasks: [
      'pytest-5227: improve the default logging format',
      'sphinx-8595: autodoc ignores an empty __all__',
      'django-11133: HttpResponse mishandles memoryview content',
      'flask-4045: raise an error when a blueprint name contains a dot',
      'pylint-7080: --recursive=y ignores ignore-paths',
    ],
  },
  {
    id: 'terminal-bench',
    name: 'Terminal-Bench',
    description: 'Hard tasks done end to end in a terminal: builds, scripts and system setup.',
    tasks: [
      'Build the Linux kernel and boot it in QEMU',
      'Recover lost changes from a broken git history',
      'Recover the password of an encrypted 7z archive',
      'Compile SQLite with gcov coverage and report it',
      'Serve a git repository that deploys to a web server on push',
    ],
  },
  {
    id: 'aider-polyglot',
    name: 'Aider Polyglot',
    description: 'Hard Exercism exercises in C++, Go, Java, JavaScript, Python and Rust.',
    tasks: [
      'Rust: implement the Forth interpreter',
      'Go: score a game of bowling',
      'Python: a zipper over a binary tree',
      'C++: a thread-safe bank account',
      'JavaScript: a reactive spreadsheet of cells',
      'Java: parse and evaluate word problems',
    ],
  },
]

function benchmarkTasks(ids: readonly string[]): EvalTask[] {
  return BENCHMARKS.filter((b) => ids.includes(b.id)).flatMap((b) =>
    b.tasks.map((title, i) => ({
      id: `${b.id}:${i}`,
      benchmark_id: b.id,
      repo: '',
      title,
      status: 'verified' as const,
      stage: 'done',
      reject_reason: null,
      original: null,
    })),
  )
}

/** Every model a dataset can offer: the sessions' own come first, these fill the comparison. */
const CATALOG: Record<string, { name: string; bias: number; cost: number; latency: number }> = {
  'claude-opus-4': { name: 'Claude Opus 4', bias: 10, cost: 0.09, latency: 9_000 },
  'claude-sonnet-4': { name: 'Claude Sonnet 4', bias: 6, cost: 0.03, latency: 5_000 },
  'gpt-5': { name: 'GPT-5', bias: 7, cost: 0.05, latency: 7_000 },
  'gpt-5-codex': { name: 'GPT-5 Codex', bias: 5, cost: 0.02, latency: 6_000 },
  'gemini-2.5-pro': { name: 'Gemini 2.5 Pro', bias: 4, cost: 0.03, latency: 6_500 },
}
const OTHER = { bias: -6, cost: 0.01, latency: 4_000 }

/** FNV-1a: a stable 32-bit hash, so each id pair always gets the same numbers. */
function hash(s: string): number {
  let h = 0x811c9dc5
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 0x01000193)
  return h >>> 0
}

const notFound = (error: string, code: string) =>
  HttpResponse.json({ error, code }, { status: 404 })
const badRequest = (error: string, code: string) =>
  HttpResponse.json({ error, code }, { status: 400 })

function sessionsOf(ctx: EvalsCtx, harness?: string): HSession[] {
  const hs = ctx.harnessSeed()
  const mine = new Set(
    hs.agents
      .filter((a) => a.owner_id === ctx.viewerId() && !a.deleted && !a.spoofed)
      .map((a) => a.id),
  )
  return hs.sessions
    .filter((s) => mine.has(s.agent_id) && (!harness || s.harness === harness))
    .sort((a, b) => b.ts - a.ts)
}

function connected(ctx: EvalsCtx): string[] {
  const hs = ctx.harnessSeed()
  const id = ctx.viewerId()
  return [
    ...new Set(
      hs.agents.filter((a) => a.owner_id === id && !a.deleted && !a.spoofed).map((a) => a.harness),
    ),
  ]
}

const titleOf = (sessionId: string) => PROMPTS[hash(sessionId) % PROMPTS.length]!
const repoOf = (sessionId: string) => (hash(sessionId) % 3 ? 'acme/platform' : 'acme/billing')

/** A session usable for evals unless its hash says otherwise: every fifth was synced before its repository was. */
function judged(
  s: HSession,
  harness: string,
): Pick<EvalSession, 'repo' | 'eligible' | 'ineligible_reason'> {
  if (harness !== 'claude')
    return { repo: repoOf(s.session_id), eligible: false, ineligible_reason: 'unsupported_harness' }
  if (hash(s.session_id) % 5 === 0)
    return { repo: null, eligible: false, ineligible_reason: 'no_repo' }
  return { repo: repoOf(s.session_id), eligible: true, ineligible_reason: null }
}

/** A session's tasks: usually verified, one in three rejected by the gate (as real sessions often are). */
function tasksOf(sessionId: string, original: string): EvalTask[] {
  const h = hash(sessionId)
  return Array.from({ length: 1 + (h % 2) }, (_, j) => {
    const rejected = (h + j) % 3 === 2
    return {
      id: `${sessionId}:${j}`,
      session_id: sessionId,
      repo: repoOf(sessionId),
      title: PROMPTS[(h + j) % PROMPTS.length]!,
      status: rejected ? 'rejected' : 'verified',
      stage: 'done',
      reject_reason: rejected ? 'gate: NoFailToPass' : null,
      original: {
        model: original,
        cost_usd: Math.round((0.2 + (h % 100) / 100) * 10_000) / 10_000,
        latency_ms: 600_000 + (h % 1_200_000),
      },
    }
  })
}

/** The viewer's LLM configs: one per catalog model. */
function modelOf(id: string, used: boolean): EvalModel {
  return {
    id,
    name: CATALOG[id]?.name ?? id,
    provider: id.startsWith('gpt') ? 'openai' : id.startsWith('gemini') ? 'gemini' : 'anthropic',
    model: id,
    used,
  }
}

function result(taskId: string, modelId: string, done: boolean): EvalResult {
  const h = hash(`${taskId}|${modelId}`)
  const m = CATALOG[modelId] ?? OTHER
  if (!done)
    return {
      task_id: taskId,
      model_id: modelId,
      attempt: 1,
      status: 'pending',
      score: null,
      resolved: null,
      cost_usd: null,
      latency_ms: null,
      failure: null,
      builder: false,
    }
  const score = h % 4 === 0 ? 100 : Math.max(0, Math.min(100, 50 + (h % 40) + m.bias))
  return {
    task_id: taskId,
    model_id: modelId,
    attempt: 1,
    status: 'done',
    score,
    resolved: score === 100,
    cost_usd: Math.round(m.cost * (0.5 + (h % 100) / 100) * 10_000) / 10_000,
    latency_ms: Math.round(m.latency * 60 * (0.6 + ((h >>> 8) % 80) / 100)),
    failure: score === 100 ? null : 'tests_failed',
    builder: false,
  }
}

interface DatasetState {
  id: string
  harness: string
  sessionIds: string[]
  benchmarkIds: string[]
  reads: number
}
interface RunState {
  id: string
  datasetId: string
  taskIds: string[]
  modelIds: string[]
  reads: number
  createdAt: string
}
let datasets = new Map<string, DatasetState>()
let runs = new Map<string, RunState>()

/** Drop datasets and runs (tests call this between cases, through the shared reset). */
export function resetEvalsMock() {
  datasets = new Map()
  runs = new Map()
}

const idOf = (prefix: string, seed: string) =>
  `5eed0017-${prefix}-4000-8000-${hash(seed).toString(16).padStart(12, '0')}`

function datasetView(ctx: EvalsCtx, d: DatasetState): EvalDataset {
  if (d.benchmarkIds.length) {
    const tasks = benchmarkTasks(d.benchmarkIds)
    // No sessions, so no model was "used": every model is offered and none is ticked.
    return {
      id: d.id,
      status: 'ready',
      progress: { done: tasks.length, total: tasks.length },
      outcomes: {},
      error: null,
      tasks,
      models: Object.keys(CATALOG).map((m) => modelOf(m, false)),
    }
  }
  const picked = sessionsOf(ctx, d.harness).filter((s) => d.sessionIds.includes(s.session_id))
  const used = [...new Set(picked.map((s) => s.model))]
  const ready = d.reads > 0
  const tasks = ready ? picked.flatMap((s) => tasksOf(s.session_id, s.model)) : []
  return {
    id: d.id,
    status: ready ? 'ready' : 'harvesting',
    progress: { done: tasks.length, total: tasks.length },
    outcomes: ready ? Object.fromEntries(picked.map((s) => [s.session_id, 'task'])) : {},
    error: null,
    tasks,
    models: [
      ...used.map((m) => modelOf(m, true)),
      ...Object.keys(CATALOG)
        .filter((m) => !used.includes(m))
        .map((m) => modelOf(m, false)),
    ],
  }
}

function runView(r: RunState): EvalRun {
  const done = r.reads > 0
  const results = r.taskIds.flatMap((t) => r.modelIds.map((m) => result(t, m, done)))
  return {
    id: r.id,
    dataset_id: r.datasetId,
    status: done ? 'done' : 'queued',
    models: r.modelIds.map((m) => ({ id: m, name: CATALOG[m]?.name ?? m })),
    progress: { done: done ? results.length : 0, total: results.length },
    results,
    error: null,
    created_at: r.createdAt,
  }
}

export function evalsHandlers(ctx: EvalsCtx): HttpHandler[] {
  // Every route checks the session first, then whether this server "has" evals at all.
  const gate = () =>
    !ctx.loggedIn()
      ? ctx.unauthorized()
      : ctx.absent()
        ? new HttpResponse(null, { status: 404 })
        : null
  return [
    http.get('/api/evals/harnesses', () => {
      const stop = gate()
      if (stop) return stop
      const harnesses = connected(ctx).map((id) => {
        const sessions = sessionsOf(ctx, id)
        return {
          id,
          sessions: sessions.length,
          last_active_at: sessions[0]?.started_at ?? null,
          supported: id === 'claude',
        }
      })
      return HttpResponse.json({
        data: {
          harnesses,
          github_connected: !ctx.githubOff(),
          chat_history: true,
          runner_available: true,
        },
      })
    }),
    http.get('/api/evals/harnesses/:id/sessions', ({ params }) => {
      const stop = gate()
      if (stop) return stop
      const id = String(params.id)
      if (!connected(ctx).includes(id))
        return notFound(`${id} is not connected`, 'harness_not_connected')
      const sessions: EvalSession[] = sessionsOf(ctx, id)
        .slice(0, SESSION_CAP)
        .map((s) => {
          const j = judged(s, id)
          return {
            session_id: s.session_id,
            title: titleOf(s.session_id),
            started_at: s.started_at,
            turns: s.turns,
            cost_usd: s.cost_usd,
            models: [s.model],
            ...(ctx.githubOff() && j.eligible
              ? { ...j, eligible: false, ineligible_reason: 'github_not_connected' as const }
              : j),
          }
        })
      return HttpResponse.json({ data: { sessions, next_cursor: null } })
    }),
    http.get('/api/evals/benchmarks', () => {
      const stop = gate()
      if (stop) return stop
      const benchmarks: EvalBenchmark[] = BENCHMARKS.map((b) => ({ ...b, tasks: b.tasks.length }))
      return HttpResponse.json({ data: { benchmarks } })
    }),
    http.post('/api/evals/datasets', async ({ request }) => {
      const stop = gate()
      if (stop) return stop
      const body = (await request.json().catch(() => ({}))) as {
        harness?: string
        session_ids?: string[]
        benchmark_ids?: string[]
      }
      const harness = body.harness ?? ''
      if (!connected(ctx).includes(harness))
        return notFound(`${harness} is not connected`, 'harness_not_connected')
      if (body.benchmark_ids) {
        const picked = BENCHMARKS.filter((b) => body.benchmark_ids?.includes(b.id)).map((b) => b.id)
        if (!picked.length) return badRequest('pick at least one benchmark', 'no_benchmarks')
        const d: DatasetState = {
          id: idOf('0000', `bench|${[...picked].sort().join()}|${datasets.size}`),
          harness,
          sessionIds: [],
          benchmarkIds: picked,
          reads: 0,
        }
        datasets.set(d.id, d)
        return HttpResponse.json({ data: datasetView(ctx, d) }, { status: 202 })
      }
      if (ctx.githubOff())
        return HttpResponse.json(
          { error: 'connect GitHub', code: 'github_not_connected' },
          { status: 409 },
        )
      const ids = [...new Set(body.session_ids ?? [])]
      if (!ids.length || ids.length > 50)
        return badRequest('pick 1 to 50 sessions', 'invalid_sessions')
      const d: DatasetState = {
        id: idOf('0000', `${harness}|${[...ids].sort().join()}|${datasets.size}`),
        harness,
        sessionIds: ids,
        benchmarkIds: [],
        reads: 0,
      }
      datasets.set(d.id, d)
      return HttpResponse.json({ data: datasetView(ctx, d) }, { status: 202 })
    }),
    http.get('/api/evals/datasets/:id', ({ params }) => {
      const stop = gate()
      if (stop) return stop
      const d = datasets.get(String(params.id))
      if (!d) return notFound('no such dataset', 'not_found')
      const view = datasetView(ctx, d)
      d.reads += 1
      return HttpResponse.json({ data: view })
    }),
    http.post('/api/evals/datasets/:id/runs', async ({ params, request }) => {
      const stop = gate()
      if (stop) return stop
      const d = datasets.get(String(params.id))
      if (!d) return notFound('no such dataset', 'not_found')
      const body = (await request.json().catch(() => ({}))) as {
        task_ids?: string[]
        model_ids?: string[]
      }
      const tasks = body.task_ids ?? []
      const models = body.model_ids ?? []
      if (!tasks.length) return badRequest('pick verified tasks of this dataset', 'invalid_tasks')
      if (!models.length) return badRequest('pick at least one model', 'invalid_model')
      const r: RunState = {
        id: idOf('0001', `${d.id}|${tasks.join()}|${models.join()}|${runs.size}`),
        datasetId: d.id,
        taskIds: tasks,
        modelIds: models,
        reads: 0,
        createdAt: new Date().toISOString(),
      }
      runs.set(r.id, r)
      return HttpResponse.json({ data: runView(r) }, { status: 202 })
    }),
    http.get('/api/evals/runs/:id', ({ params }) => {
      const stop = gate()
      if (stop) return stop
      const r = runs.get(String(params.id))
      if (!r) return notFound('no such run', 'not_found')
      const view = runView(r)
      r.reads += 1
      return HttpResponse.json({ data: view })
    }),
    http.get('/api/evals/runs', () => {
      const stop = gate()
      if (stop) return stop
      const list = [...runs.values()].reverse().map((r) => {
        const v = runView(r)
        return {
          id: v.id,
          dataset_id: v.dataset_id,
          harness: datasets.get(r.datasetId)?.harness ?? '',
          status: v.status,
          models: v.models,
          created_at: v.created_at,
        }
      })
      return HttpResponse.json({ data: list })
    }),
  ]
}
