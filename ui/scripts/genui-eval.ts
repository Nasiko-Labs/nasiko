import { spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium, type Browser, type Request } from '@playwright/test'
import { runnerImport } from 'vite'
import { auditLayout, layoutProblems } from '../ee/web/src/genui/eval/layout-audit.ts'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const EVAL = join(ROOT, 'ee/web/src/genui/eval')
const BASELINE = join(EVAL, 'baseline.json')
const CANDIDATE = join(EVAL, 'candidate.json')
const REQUESTS = join(EVAL, 'requests.json')
const SHOTS = join(ROOT, process.env.GENUI_EVAL_OUT ?? 'genui-eval')
const BASE = process.env.GENUI_EVAL_BASE ?? 'http://localhost:8081'
const MAX_CALLS = Number(process.env.LIVE_MAX_CALLS)
const CONTAINER = process.env.GENUI_EVAL_WEAVE_CONTAINER
const REPAIR = /^Fix these issues/
const SETTLE_MS = 6000
const TURN_DEADLINE_MS = 240_000
const BUSY = [
  'Building your view…',
  'Improving the layout…',
  'Building a custom view',
  'Checking this view…',
]

interface Usage {
  skill: string
  input: number
  output: number
  cacheRead: number
  cacheCreation: number
}

interface Call {
  skill: string
  prompt: string
  status: number
  ms: number
  chars: number
  usedCustom: boolean
  usage?: Usage
}

interface FirstTry {
  parseErrors: number
  findings: string[]
  repairFlagged: number
}

interface Result {
  id: string
  prompt: string
  totalSeconds: number
  calls: Call[]
  screenBuilt: boolean
  generatedViews: number
  viewsFailed: number
  viewsDeclined: number
  viewsIncomplete: number
  blockedByBudget: number
  firstTry: FirstTry
  repairTurns: number
  prose: string[]
  consoleErrors: string[]
  programs: string[]
  components: string[]
  layoutIssues?: string[]
  bands?: string[]
}

interface Baseline {
  recordedAt: string
  source: string
  promptTokens: { catalog: number; react: number }
  requests: Result[]
}

interface Finding {
  rule: string
  message: string
  repair?: boolean
}

if (!Number.isInteger(MAX_CALLS) || MAX_CALLS < 1)
  throw new Error('set LIVE_MAX_CALLS to the number of model calls this run may spend')
const username = process.env.ADMIN_USERNAME
const password = process.env.ADMIN_PASSWORD
if (!username || !password) throw new Error('set ADMIN_USERNAME and ADMIN_PASSWORD')

const resolve = {
  alias: {
    '@ee': join(ROOT, 'ee/web/src'),
    '@edition': join(ROOT, 'ee/web/src'),
    '@': join(ROOT, 'common/src'),
  },
}
const load = async <T>(file: string) =>
  (await runnerImport<T>(join(ROOT, file), { configFile: false, logLevel: 'error', resolve }))
    .module
const { lintProgram } = await load<{ lintProgram: (p: string) => Finding[] }>(
  'ee/web/src/genui/lint/index.ts',
)
const { layoutDiversity, layoutSignature } = await load<{
  layoutDiversity: (programs: string[]) => { distinct: number; of: number }
  layoutSignature: (program: string) => string
}>('ee/web/src/genui/eval/layout.ts')
const { programOf, proseOf } = await load<{
  programOf: (text: string) => string
  proseOf: (text: string) => string
}>('ee/web/src/genui/conversation/prose.ts')

function firstTry(text: string | undefined): FirstTry {
  const program = text ? programOf(text) : null
  const findings = program ? lintProgram(program) : []
  return {
    parseErrors: findings.filter((f) => f.rule === 'L12').length,
    findings: findings.map((f) => `${f.rule}${f.repair ? '*' : ''}: ${f.message}`),
    repairFlagged: findings.filter((f) => f.repair).length,
  }
}

const baseline = JSON.parse(readFileSync(BASELINE, 'utf8')) as Baseline
const turnsOf = (r: Result) =>
  r.calls
    .filter((c) => c.skill === 'generate-openui' && !REPAIR.test(c.prompt))
    .map((c) => c.prompt)
const v2 = JSON.parse(readFileSync(REQUESTS, 'utf8')) as {
  requests: { id: string; turns: string[] }[]
}
const fullTurns = new Map([
  ...baseline.requests.map(
    (r) => [r.id, turnsOf(r).map((turn, i) => (i === 0 ? r.prompt : turn))] as const,
  ),
  ...v2.requests.map((r) => [r.id, r.turns] as const),
])

let used = 0

function usageSince(since: string): Usage[] {
  if (!CONTAINER) return []
  const run = spawnSync('docker', ['logs', '--since', since, CONTAINER], { encoding: 'utf8' })
  const logs = `${run.stdout}${run.stderr}`
  return [
    ...logs.matchAll(
      /genui_openui\.(skill|react_skill): usage: input=(\d+) output=(\d+) cache_read=(\d+) cache_creation=(\d+)/g,
    ),
  ].map((m) => ({
    skill: m[1] === 'skill' ? 'generate-openui' : 'generate-react',
    input: Number(m[2]),
    output: Number(m[3]),
    cacheRead: Number(m[4]),
    cacheCreation: Number(m[5]),
  }))
}

async function runRequest(browser: Browser, id: string, turns: string[]): Promise<Result> {
  const since = new Date().toISOString()
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } })
  const login = await context.request.post(`${BASE}/api/auth/login`, {
    data: { username, password },
  })
  if (!login.ok()) throw new Error(`login failed: ${login.status()}`)
  const page = await context.newPage()
  const calls: (Call & { text: string })[] = []
  const consoleErrors = new Set<string>()
  const started = new Map<Request, number>()
  let inflight = 0
  let blocked = 0
  let lastActivity = Date.now()

  page.on('console', (m) => {
    if (m.type() === 'error') consoleErrors.add(m.text().slice(0, 200))
  })
  page.on('pageerror', (e) => consoleErrors.add(e.message.slice(0, 200)))
  await page.route('**/api/weave/surface', async (route) => {
    if (used >= MAX_CALLS) {
      blocked++
      await route.abort()
      return
    }
    used++
    inflight++
    started.set(route.request(), Date.now())
    lastActivity = Date.now()
    await route.continue()
  })
  page.on('response', async (response) => {
    if (!response.url().endsWith('/api/weave/surface')) return
    const request = response.request()
    const body = JSON.parse(request.postData() ?? '{}') as { skill: string; prompt: string }
    const raw = await response.text().catch(() => '')
    const text = [...raw.matchAll(/event: dsl-chunk\ndata: (\{.*\})/g)]
      .map((m) => (JSON.parse(m[1] ?? '{}') as { text?: string }).text ?? '')
      .join('')
    calls.push({
      skill: body.skill,
      prompt: body.prompt.slice(0, 90),
      status: response.status(),
      ms: Date.now() - (started.get(request) ?? Date.now()),
      chars: text.length,
      usedCustom: /\bCustom\(/.test(text),
      text,
    })
    inflight--
    lastActivity = Date.now()
  })

  const composer = page.getByRole('textbox', { name: 'Ask Weave' })
  await page.goto(`${BASE}/tokenops`)
  await page.getByRole('button', { name: /^Open Weave/ }).click()
  const t0 = Date.now()
  for (const turn of turns) {
    await composer.fill(turn)
    await composer.press('Enter')
    await page.waitForTimeout(2500)
    const deadline = Date.now() + TURN_DEADLINE_MS
    while (Date.now() < deadline) {
      await page.waitForTimeout(1000)
      const busy = await Promise.all(BUSY.map((t) => page.getByText(t).count()))
      if (inflight === 0 && busy.every((n) => n === 0) && Date.now() - lastActivity > SETTLE_MS)
        break
    }
  }
  const totalSeconds = Math.round((Date.now() - t0) / 1000)

  const canvas = page.getByRole('region', { name: 'Generated view' })
  const grid = canvas.locator('[data-genui-canvas]')
  const capture = async (file: string) => {
    const height = await page.evaluate(() => document.body.scrollHeight)
    for (let y = 0; y < height; y += 600) {
      await page.evaluate((top) => window.scrollTo(0, top), y)
      await page.waitForTimeout(500)
    }
    await page.evaluate(() => window.scrollTo(0, 0))
    const viewport = page.viewportSize() ?? { width: 1440, height: 1000 }
    await page.setViewportSize({
      width: viewport.width,
      height: await page.evaluate(() => document.body.scrollHeight),
    })
    await page.waitForTimeout(2500)
    await page.screenshot({ path: join(SHOTS, file) })
    await page.setViewportSize(viewport)
    if (!(await grid.count())) return { issues: [], bands: '' }
    const audit = await grid.evaluate(auditLayout)
    return {
      issues: layoutProblems(audit),
      bands: `${audit.width}px ${audit.bands.map((b) => `${b.kind}:${b.sections.length}`).join(' ')}`,
    }
  }
  const narrow = await capture(`${id}.png`)
  await page.getByRole('button', { name: 'Hide chat' }).click()
  const wide = await capture(`${id}-wide.png`)
  const layoutIssues = [...narrow.issues, ...wide.issues.map((i) => `wide: ${i}`)]

  const usage = usageSince(since)
  for (const skill of ['generate-openui', 'generate-react']) {
    const lines = usage.filter((u) => u.skill === skill)
    calls.filter((c) => c.skill === skill).forEach((c, i) => (c.usage = lines[i]))
  }
  const openui = calls.filter((c) => c.skill === 'generate-openui')
  const result: Result = {
    id,
    prompt: turns[0] ?? '',
    totalSeconds,
    calls: calls.map(({ text: _text, ...call }) => call),
    screenBuilt: openui.some((c) => programOf(c.text).trim() !== ''),
    generatedViews: await canvas.locator('iframe[title="Generated view"]').count(),
    viewsFailed: await canvas.getByText('could not be built').count(),
    viewsDeclined: await canvas.getByText('This part could not be drawn').count(),
    viewsIncomplete: await canvas.getByText('may be incomplete').count(),
    blockedByBudget: blocked,
    firstTry: firstTry(openui[0]?.text),
    repairTurns: openui.filter((c) => REPAIR.test(c.prompt)).length,
    prose: openui.filter((c) => !REPAIR.test(c.prompt)).map((c) => proseOf(c.text)),
    consoleErrors: [...consoleErrors].slice(0, 5),
    programs: openui.map((c) => c.text),
    components: calls.filter((c) => c.skill === 'generate-react').map((c) => c.text),
    layoutIssues,
    bands: [narrow.bands, wide.bands],
  }
  await context.close()
  return result
}

const promptTokens = (requests: Result[], skill: string) =>
  Math.max(
    0,
    ...requests.flatMap((r) =>
      r.calls
        .filter((c) => c.skill === skill && c.usage)
        .map((c) => (c.usage?.cacheCreation || c.usage?.cacheRead) ?? 0),
    ),
  )

function totals(requests: Result[], relint: boolean) {
  const tries = requests.map((r) => (relint ? firstTry(r.programs[0]) : r.firstTry))
  const openui = requests.flatMap((r) => r.calls.filter((c) => c.skill === 'generate-openui'))
  return {
    requests: requests.length,
    calls: requests.reduce((n, r) => n + r.calls.length, 0),
    parseErrors: tries.reduce((n, t) => n + t.parseErrors, 0),
    repairFlagged: tries.reduce((n, t) => n + t.repairFlagged, 0),
    repairRate: requests.filter((r) => r.repairTurns > 0).length / (requests.length || 1),
    screensBuilt: requests.filter((r) => r.screenBuilt).length,
    avgCatalogSeconds:
      Math.round(openui.reduce((n, c) => n + c.ms, 0) / (openui.length || 1) / 100) / 10,
    outputTokens: openui.reduce((n, c) => n + (c.usage?.output ?? 0), 0),
  }
}

function report(candidate: Result[]) {
  const ids = candidate.map((r) => r.id)
  const base = baseline.requests.filter((r) => ids.includes(r.id))
  const rows = candidate.map((c) => {
    const b = base.find((r) => r.id === c.id)
    return {
      id: c.id,
      'calls b/c': `${b?.calls.length ?? '-'}/${c.calls.length}`,
      'flagged b/b*/c': `${b?.firstTry.repairFlagged ?? '-'}/${b ? firstTry(b.programs[0]).repairFlagged : '-'}/${c.firstTry.repairFlagged}`,
      'repairs b/c': `${b?.repairTurns ?? '-'}/${c.repairTurns}`,
      'built b/c': `${b?.screenBuilt ?? '-'}/${c.screenBuilt}`,
      'views b/c': `${b?.generatedViews ?? '-'}/${c.generatedViews}`,
      'seconds b/c': `${b?.totalSeconds ?? '-'}/${c.totalSeconds}`,
      blocked: c.blockedByBudget,
      layout: layoutSignature(programOf(c.programs[0] ?? '')),
      'layout issues': c.layoutIssues?.length ?? '-',
    }
  })
  console.table(rows)
  console.table({
    baseline: totals(base, false),
    'baseline relinted': totals(base, true),
    candidate: totals(candidate, false),
  })
  const diversity = layoutDiversity(candidate.map((c) => programOf(c.programs[0] ?? '')))
  console.log(`layout diversity: ${diversity.distinct} distinct of ${diversity.of} screens`)
  console.log(
    `prompt tokens (Claude count) baseline catalog ${baseline.promptTokens.catalog} react ${baseline.promptTokens.react}; candidate catalog ${promptTokens(candidate, 'generate-openui') || 'n/a'} react ${promptTokens(candidate, 'generate-react') || 'n/a'}`,
  )
}

const requested = process.argv.slice(2)
const ids = requested.length ? requested : [...fullTurns.keys()]
for (const id of ids) if (!fullTurns.has(id)) throw new Error(`unknown request ${id}`)
mkdirSync(SHOTS, { recursive: true })

const previous: Result[] = existsSync(CANDIDATE)
  ? (JSON.parse(readFileSync(CANDIDATE, 'utf8')) as { requests: Result[] }).requests
  : []
const browser = await chromium.launch({ channel: 'chrome' })
const fresh: Result[] = []
for (const id of ids) {
  const result = await runRequest(browser, id, fullTurns.get(id) ?? [])
  fresh.push(result)
  console.log(`${id}: ${result.calls.length} calls, ${used}/${MAX_CALLS} used`)
}
await browser.close()

const requests = [...previous.filter((r) => !ids.includes(r.id)), ...fresh].sort(
  (a, b) => [...fullTurns.keys()].indexOf(a.id) - [...fullTurns.keys()].indexOf(b.id),
)
writeFileSync(
  CANDIDATE,
  `${JSON.stringify(
    {
      recordedAt: new Date().toISOString(),
      source: `Weave workspace at ${BASE} via scripts/genui-eval.ts`,
      promptTokens: {
        catalog: promptTokens(requests, 'generate-openui'),
        react: promptTokens(requests, 'generate-react'),
      },
      requests,
    },
    null,
    1,
  )}\n`,
)
report(requests)
