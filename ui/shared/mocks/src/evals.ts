/**
 * The proposed evals routes (docs/superpowers/specs/2026-11-23-evals-design.md), mocked under the `evals` MOCKABLE
 * group. No server has them, so `?mock=evals-absent` answers the bare 404 one without them gives.
 *
 * Sessions come from the harness seed (the viewer's live harness agents and their sessions), so Evals agrees with
 * Coding harnesses: `no-harnesses` disconnects the harness (a coded 404), `no-activity` leaves it connected with no
 * sessions, which is when the page offers the benchmarks. Tasks, scores, costs and latencies are derived from a hash of
 * the ids, so the same picks always give the same table. Nothing is stored: a run needs only the ids.
 */
import { delay, http, HttpResponse, type HttpHandler } from 'msw'
import type {
  EvalBenchmark,
  EvalDataset,
  EvalModel,
  EvalResult,
  EvalSession,
  EvalTask,
} from '@nasiko/features/evals/types'
import { providerOfModel, type HarnessSeed, type HSession } from './seed-harness'

export interface EvalsCtx {
  loggedIn: () => boolean
  unauthorized: () => Response
  /** `?mock=evals-absent`. */
  absent: () => boolean
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

function sessionsOf(ctx: EvalsCtx, harness: string): HSession[] {
  const hs = ctx.harnessSeed()
  const mine = new Set(
    hs.agents
      .filter((a) => a.owner_id === ctx.viewerId() && !a.deleted && !a.spoofed)
      .map((a) => a.id),
  )
  return hs.sessions
    .filter((s) => mine.has(s.agent_id) && s.harness === harness)
    .sort((a, b) => b.ts - a.ts)
}

const connected = (ctx: EvalsCtx, harness: string) =>
  ctx
    .harnessSeed()
    .agents.some(
      (a) => a.owner_id === ctx.viewerId() && !a.deleted && !a.spoofed && a.harness === harness,
    )

const titleOf = (sessionId: string) => PROMPTS[hash(sessionId) % PROMPTS.length]!

function sessionTasks(sessionId: string): EvalTask[] {
  const h = hash(sessionId)
  return Array.from({ length: 2 + (h % 2) }, (_, j) => ({
    id: `${sessionId}:${j}`,
    session_id: sessionId,
    title: PROMPTS[(h + j) % PROMPTS.length]!,
  }))
}

function modelOf(id: string, used: boolean): EvalModel {
  return { id, name: CATALOG[id]?.name ?? id, provider: providerOfModel(id), used }
}

const datasetId = (ids: Iterable<string>) =>
  `5eed0017-0000-4000-8000-${hash([...ids].sort().join())
    .toString(16)
    .padStart(12, '0')}`

function result(taskId: string, modelId: string): EvalResult {
  const h = hash(`${taskId}|${modelId}`)
  const m = CATALOG[modelId] ?? OTHER
  return {
    task_id: taskId,
    model_id: modelId,
    score: Math.max(0, Math.min(100, 50 + (h % 40) + m.bias)),
    cost_usd: Math.round(m.cost * (0.5 + (h % 100) / 100) * 10_000) / 10_000,
    latency_ms: Math.round(m.latency * (0.6 + ((h >>> 8) % 80) / 100)),
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
    http.get('/api/evals/harnesses/:id/sessions', ({ params }) => {
      const stop = gate()
      if (stop) return stop
      const id = String(params.id)
      if (!connected(ctx, id)) return notFound(`${id} is not connected`, 'harness_not_connected')
      const sessions: EvalSession[] = sessionsOf(ctx, id)
        .slice(0, SESSION_CAP)
        .map((s) => ({
          session_id: s.session_id,
          title: titleOf(s.session_id),
          started_at: s.started_at,
          turns: s.turns,
          cost_usd: s.cost_usd,
          models: [s.model],
        }))
      return HttpResponse.json({ data: { sessions } })
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
      if (!connected(ctx, harness))
        return notFound(`${harness} is not connected`, 'harness_not_connected')

      if (body.benchmark_ids) {
        const picked = BENCHMARKS.filter((b) => body.benchmark_ids?.includes(b.id))
        if (!picked.length) return badRequest('pick at least one benchmark', 'no_benchmarks')
        const dataset: EvalDataset = {
          id: datasetId(picked.map((b) => b.id)),
          tasks: picked.flatMap((b) =>
            b.tasks.map((title, i) => ({ id: `${b.id}:${i}`, benchmark_id: b.id, title })),
          ),
          // No sessions, so no model was "used": every model is offered and none is ticked.
          models: Object.keys(CATALOG).map((m) => modelOf(m, false)),
        }
        return HttpResponse.json({ data: dataset }, { status: 201 })
      }

      const ids = new Set(body.session_ids ?? [])
      const picked = sessionsOf(ctx, harness).filter((s) => ids.has(s.session_id))
      if (!picked.length) return badRequest('pick at least one session', 'no_sessions')
      const used = [...new Set(picked.map((s) => s.model))]
      const dataset: EvalDataset = {
        id: datasetId(ids),
        // The same prompt from several sessions is one task.
        tasks: picked
          .flatMap((s) => sessionTasks(s.session_id))
          .filter((t, i, all) => all.findIndex((x) => x.title === t.title) === i),
        models: [
          ...used.map((m) => modelOf(m, true)),
          ...Object.keys(CATALOG)
            .filter((m) => !used.includes(m))
            .map((m) => modelOf(m, false)),
        ],
      }
      return HttpResponse.json({ data: dataset }, { status: 201 })
    }),
    http.post('/api/evals/datasets/:id/runs', async ({ request }) => {
      const stop = gate()
      if (stop) return stop
      const body = (await request.json().catch(() => ({}))) as {
        task_ids?: string[]
        model_ids?: string[]
      }
      const tasks = body.task_ids ?? []
      const models = body.model_ids ?? []
      if (!tasks.length || !models.length)
        return badRequest('pick at least one task and one model', 'empty_selection')
      // Realistic in the browser, instant under Node (tests).
      await delay()
      return HttpResponse.json({
        data: { results: tasks.flatMap((t) => models.map((m) => result(t, m))) },
      })
    }),
  ]
}
