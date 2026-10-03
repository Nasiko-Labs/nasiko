/**
 * Routed chat smoke (v1b plan §5.14, X6): drives the same HTTP flow as the UI against a live
 * OpenRuntime server and proves the server half of the contract:
 * - the orchestrator calls the expected agent (a `tool_call` naming it);
 * - the server saves exactly one assistant row per turn, with the turn's trace id;
 * - this client never POSTs an assistant row.
 * A raw HTTP/SSE recorder: it imports nothing from src/. All interpretation of the frames
 * happens in the Vitest replay of the captured fixture.
 *
 *   node scripts/smoke-chat-routed.ts [--runs 5] [--record] [--cluster <name>] [--keep]
 */
import { readFileSync, writeFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { parseArgs as parseArgv } from 'node:util'
import { Failure, isLoopbackUrl, SECRET } from './lib/live.ts'

const FIXTURE = 'common/src/features/chat/__fixtures__/live-routed.json'
const DEFAULT_PROMPT = 'Use the currency agent to convert 100 USD to EUR.'
const DEFAULT_AGENT = 'currency-agent'
const TIMEOUT = {
  preflight: 5_000,
  create: 30_000,
  request: 15_000,
  stream: 120_000,
  history: 10_000,
}

const HELP = `Routed chat smoke (v1b plan §5.14)

Usage: node scripts/smoke-chat-routed.ts [options]

Options:
  --runs <n>         Run the turn n times (default 1; the release bar is 5 of 5)
  --record           Write the scrubbed stream to ${FIXTURE} (verification runs never write it)
  --cluster <name>   Use this cluster from ~/.nasiko/config.json
  --keep             Keep the chats it creates (they're deleted by default)
  --agent <name>     The agent the orchestrator must call (default ${DEFAULT_AGENT})
  --prompt <text>    The prompt to send (default "${DEFAULT_PROMPT}")
  --help             Show this help

Auth, first match wins:
  1. NASIKO_URL and NASIKO_TOKEN
  2. --cluster <name>: clusters[name].{url,token} in ~/.nasiko/config.json
  3. ~/.nasiko/config.json "active" cluster

Needs the "${DEFAULT_AGENT}" agent running (nasiko upload oss/agents/currency-agent), or
--agent and --prompt naming one that is.

Example:
  node scripts/smoke-chat-routed.ts --runs 5
`

interface Args {
  runs: number
  record: boolean
  cluster?: string
  keep: boolean
  agent: string
  prompt: string
}

function parseArgs(argv: string[]): Args | 'help' {
  let values
  try {
    ;({ values } = parseArgv({
      args: argv,
      strict: true,
      options: {
        help: { type: 'boolean', short: 'h' },
        record: { type: 'boolean' },
        keep: { type: 'boolean' },
        runs: { type: 'string' },
        cluster: { type: 'string' },
        agent: { type: 'string' },
        prompt: { type: 'string' },
      },
    }))
  } catch (err) {
    throw new Failure('args', `${(err as Error).message} (see --help)`)
  }
  if (values.help) return 'help'
  return {
    runs: Math.max(1, Number(values.runs) || 1),
    record: !!values.record,
    keep: !!values.keep,
    cluster: values.cluster,
    agent: values.agent ?? DEFAULT_AGENT,
    prompt: values.prompt ?? DEFAULT_PROMPT,
  }
}

interface Auth {
  url: string
  token: string
  source: string
}

function resolveAuth(cluster?: string): Auth {
  const envUrl = process.env.NASIKO_URL
  const envToken = process.env.NASIKO_TOKEN
  if (envUrl && envToken) return { url: envUrl, token: envToken, source: 'NASIKO_URL/NASIKO_TOKEN' }
  let config: { active?: string; clusters?: Record<string, { url?: string; token?: string }> }
  try {
    config = JSON.parse(readFileSync(join(homedir(), '.nasiko', 'config.json'), 'utf8'))
  } catch {
    throw new Failure(
      'auth',
      'no NASIKO_URL/NASIKO_TOKEN and no readable ~/.nasiko/config.json; run `nasiko connect <url>` then `nasiko auth login`',
    )
  }
  const name = cluster ?? config.active
  const c = name ? config.clusters?.[name] : undefined
  if (!name || !c?.url || !c.token)
    throw new Failure(
      'auth',
      `cluster '${name ?? '(none active)'}' has no url/token in ~/.nasiko/config.json; run \`nasiko auth login\``,
    )
  const url = /^https?:\/\//.test(c.url) ? c.url : `${isLocal(c.url) ? 'http' : 'https'}://${c.url}`
  return { url, token: c.token, source: `~/.nasiko/config.json (${name})` }
}

/** Loopback hosts, where a plain-HTTP token never leaves the machine (exact names, scripts/lib/live.ts). */
function isLocal(urlOrHost: string): boolean {
  return isLoopbackUrl(/^https?:\/\//.test(urlOrHost) ? urlOrHost : `http://${urlOrHost}`)
}

interface Sent {
  method: string
  path: string
  role?: string
}

class Client {
  readonly sent: Sent[] = []
  private readonly auth: Auth
  constructor(auth: Auth) {
    this.auth = auth
  }

  async request(
    stage: string,
    method: string,
    path: string,
    body: unknown,
    timeoutMs: number,
  ): Promise<Response> {
    const role =
      body && typeof body === 'object' && 'role' in body
        ? String((body as { role: unknown }).role)
        : undefined
    this.sent.push({ method, path, role })
    let res: Response
    try {
      res = await fetch(new URL(path, this.auth.url), {
        method,
        headers: {
          Authorization: `Bearer ${this.auth.token}`,
          'Content-Type': 'application/json',
          Accept: path.endsWith('/a2a') ? 'text/event-stream' : 'application/json',
        },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(timeoutMs),
      })
    } catch (err) {
      const name = (err as { name?: string }).name
      throw new Failure(
        stage,
        name === 'TimeoutError'
          ? `timed out after ${timeoutMs / 1000} s`
          : `network error: ${(err as Error).message}`,
      )
    }
    if (res.status === 401)
      throw new Failure(
        'auth',
        'Token expired or invalid; run `nasiko auth login` or `nasiko connect`.',
      )
    return res
  }

  async json<T>(
    stage: string,
    method: string,
    path: string,
    body: unknown,
    timeoutMs: number,
  ): Promise<T> {
    const res = await this.request(stage, method, path, body, timeoutMs)
    const text = await res.text()
    if (!res.ok)
      throw new Failure(stage, `${stage} failed: HTTP ${res.status} ${text.slice(0, 200)}`)
    return (text ? JSON.parse(text) : null) as T
  }
}

/**
 * Every `data:` payload of an SSE body, parsed where it's JSON. The dispatch's
 * `AbortSignal.timeout` also bounds the body: a read past it rejects with TimeoutError.
 */
async function readStream(res: Response, timeoutMs: number): Promise<unknown[]> {
  const frames: unknown[] = []
  if (!res.body) return frames
  const reader = res.body.getReader()
  const dec = new TextDecoder()
  let buf = ''
  for (;;) {
    let next: ReadableStreamReadResult<Uint8Array>
    try {
      next = await reader.read()
    } catch (err) {
      const name = (err as { name?: string }).name
      if (name === 'TimeoutError' || name === 'AbortError')
        throw new Failure('stream', `stream timed out after ${timeoutMs / 1000} s`)
      throw new Failure('stream', `stream failed: ${(err as Error).message}`)
    }
    if (next.done) break
    buf += dec.decode(next.value, { stream: true })
    const events = buf.replace(/\r\n?/g, '\n').split('\n\n')
    buf = events.pop() ?? ''
    for (const ev of events) {
      const data = ev
        .split('\n')
        .filter((l) => l.startsWith('data:'))
        .map((l) => l.slice(5).trimStart())
        .join('\n')
      if (!data) continue
      try {
        frames.push(JSON.parse(data))
      } catch {
        frames.push({ raw: data })
      }
    }
  }
  return frames
}

/** Data parts anywhere in a frame (statusUpdate messages), as the server nests them. */
function dataParts(frame: unknown): Record<string, unknown>[] {
  const out: Record<string, unknown>[] = []
  const walk = (v: unknown) => {
    if (!v || typeof v !== 'object') return
    if (Array.isArray(v)) return v.forEach(walk)
    const o = v as Record<string, unknown>
    if (
      o.data &&
      typeof o.data === 'object' &&
      typeof (o.data as { type?: unknown }).type === 'string'
    )
      out.push(o.data as Record<string, unknown>)
    for (const x of Object.values(o)) walk(x)
  }
  walk(frame)
  return out
}

function artifactText(frames: unknown[]): string {
  let text = ''
  for (const f of frames) {
    const u = ((f as { result?: unknown }).result ?? f) as {
      artifactUpdate?: { artifact?: { parts?: { text?: string }[] }; append?: boolean }
    }
    const parts = u.artifactUpdate?.artifact?.parts
    if (!parts) continue
    const t = parts.map((p) => p.text ?? '').join('')
    text += t
  }
  return text
}

interface Run {
  sessionId: string
  traceId: string | null
  frames: unknown[]
  agents: string[]
}

async function runOnce(client: Client, args: Args): Promise<Run> {
  const { keep, prompt: PROMPT, agent: EXPECTED_AGENT } = args
  const sessionId = crypto.randomUUID()
  let traceId: string | null = null
  try {
    await client.json(
      'create',
      'POST',
      '/api/chat/sessions',
      { session_id: sessionId, first_prompt: PROMPT },
      TIMEOUT.create,
    )
    const user = await client.json<{ id?: string; data?: { id?: string } }>(
      'user-row',
      'POST',
      `/api/chat/sessions/${sessionId}/messages`,
      { role: 'user', content: PROMPT },
      TIMEOUT.request,
    )
    const userId = user?.id ?? user?.data?.id
    const res = await client.request(
      'dispatch',
      'POST',
      '/api/orchestrator/a2a',
      {
        jsonrpc: '2.0',
        id: crypto.randomUUID(),
        method: 'message/stream',
        params: {
          message: {
            messageId: crypto.randomUUID(),
            role: 'ROLE_USER',
            parts: [{ text: PROMPT }],
            contextId: sessionId,
          },
          metadata: { session_id: sessionId },
        },
      },
      TIMEOUT.stream,
    )
    if (!res.ok)
      throw new Failure(
        'dispatch',
        `dispatch failed: HTTP ${res.status} ${(await res.text()).slice(0, 200)}`,
      )
    const frames = await readStream(res, TIMEOUT.stream)
    const parts = frames.flatMap(dataParts)
    traceId = (parts.find((p) => p.type === 'trace_meta')?.trace_id as string | undefined) ?? null
    const agents = parts.filter((p) => p.type === 'tool_call').map((p) => String(p.agent))
    if (!agents.includes(EXPECTED_AGENT))
      throw new Failure(
        'agent',
        `expected a tool_call to ${EXPECTED_AGENT}, got ${agents.length ? agents.join(', ') : 'none'} (the orchestrator answered itself or picked another agent)`,
      )
    if (!artifactText(frames).trim()) throw new Failure('empty', 'orchestrator returned no text')
    // The server saves at Done; give it a moment, then count.
    const deadline = Date.now() + TIMEOUT.history
    let assistants: { trace_id?: string | null }[] = []
    for (;;) {
      const page = await client.json<{
        data?: { id: string; role: string; trace_id?: string | null }[]
      }>(
        'history',
        'GET',
        `/api/chat/sessions/${sessionId}/messages?limit=100`,
        undefined,
        TIMEOUT.request,
      )
      const rows = page?.data ?? []
      const after = userId ? rows.slice(rows.findIndex((r) => r.id === userId) + 1) : rows
      assistants = after.filter((r) => r.role === 'assistant')
      if (assistants.length >= 1 || Date.now() > deadline) break
      await new Promise((r) => setTimeout(r, 500))
    }
    if (assistants.length !== 1)
      throw new Failure('rows', `expected exactly one assistant row, found ${assistants.length}`)
    if (traceId && assistants[0]!.trace_id !== traceId)
      throw new Failure(
        'rows',
        `the saved row's trace id ${assistants[0]!.trace_id ?? 'null'} isn't the turn's ${traceId}`,
      )
    const clientAssistant = client.sent.filter(
      (s) => s.method === 'POST' && /\/messages$/.test(s.path) && s.role === 'assistant',
    )
    if (clientAssistant.length) throw new Failure('client', 'this client POSTed an assistant row')
    return { sessionId, traceId, frames, agents }
  } catch (err) {
    if (err instanceof Failure)
      err.message = `${err.message} [chat ${sessionId}${traceId ? `, trace ${traceId}` : ''}]`
    throw err
  } finally {
    if (!keep)
      await client
        .request('cleanup', 'DELETE', `/api/chat/sessions/${sessionId}`, undefined, TIMEOUT.request)
        .catch(() => undefined)
  }
}

/**
 * The only string fields the fixture keeps verbatim (an allowlist: an unknown field is scrubbed,
 * so a new server field can't leak agent or LLM text). Frame and part discriminators, agent,
 * tool and model names, enums, timestamps, and ids (which `scrub` also swaps for placeholders).
 * Mirrored by src/features/chat/live-routed.fixture.test.ts.
 */
export const KEEP = new Set([
  // Discriminators and enums.
  'type',
  'kind',
  'state',
  'role',
  'method',
  'jsonrpc',
  'status',
  'success',
  'code',
  // Names.
  'agent',
  'via_agent',
  'agent_name',
  'caller_agent_name',
  'tool',
  'model',
  // Ids (placeholders by the time they're kept) and times.
  'id',
  'taskId',
  'contextId',
  'messageId',
  'artifactId',
  'task_id',
  'context_id',
  'trace_id',
  'traceId',
  'session_id',
  'sessionId',
  'span_id',
  'timestamp',
])

/** Object keys kept as they are: short identifiers. Anything else becomes `k-<n>`. */
export const KEY_SHAPE = /^(?:[A-Za-z_][A-Za-z0-9_]{0,39}|k-\d+)$/
/** The placeholders `scrub` writes: ids, and the length class of a scrubbed string. */
export const PLACEHOLDER = /^(?:(?:id|hex)-\d+|session-1|trace-1|<[\w-]+ \d+ chars>)$/

/** Likely credentials; the fixture writer refuses to write any (a second line of defence). Shared with the recorder. */
export { SECRET }

/**
 * The fixture keeps the stream's shape, never its content (NE-6): ids become stable
 * placeholders, strings under a `KEEP` key stay (capped at 200 chars), and every other
 * non-empty string becomes `<key N chars>`. Numbers and booleans stay.
 */
export function scrub(
  frames: unknown[],
  ids: { sessionId: string; traceId: string | null },
): unknown[] {
  const map = new Map<string, string>()
  const place = (v: string, kind: string) => {
    if (!map.has(v)) map.set(v, `${kind}-${map.size + 1}`)
    return map.get(v)!
  }
  if (ids.traceId) map.set(ids.traceId, 'trace-1')
  map.set(ids.sessionId, 'session-1')
  const cut = (s: string) => (s.length > 200 ? `${s.slice(0, 200)}…` : s)
  // Keys are data too (an agent's data part can use free text as a key): keep identifier-shaped ones.
  let keys = 0
  const scrubKey = (k: string) => (KEY_SHAPE.test(k) ? k : `k-${++keys}`)
  const walk = (v: unknown, key = ''): unknown => {
    if (typeof v === 'string') {
      let s = v
      for (const [real, fake] of map) s = s.split(real).join(fake)
      s = s
        .replace(/\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi, (m) =>
          place(m, 'id'),
        )
        .replace(/\b[0-9a-f]{32}\b/gi, (m) => place(m, 'hex'))
      if (KEEP.has(key) || !s) return cut(s)
      return `<${key || 'value'} ${s.length} chars>`
    }
    if (Array.isArray(v)) return v.map((x) => walk(x, key))
    if (v && typeof v === 'object')
      return Object.fromEntries(
        Object.entries(v)
          .filter(([k]) => !/^(authorization|cookie|set-cookie|token)$/i.test(k))
          .map(([k, x]) => {
            const key = scrubKey(k)
            return [key, walk(x, key)]
          }),
      )
    return v
  }
  return frames.map((f) => walk(f))
}

async function main() {
  let args: Args | 'help'
  try {
    args = parseArgs(process.argv.slice(2))
  } catch (err) {
    console.error(`smoke: ${(err as Error).message}`)
    process.exit(2)
  }
  if (args === 'help') {
    process.stdout.write(HELP)
    return
  }
  const auth = resolveAuth(args.cluster)
  console.log(`smoke: ${auth.url} (auth from ${auth.source})`)
  if (auth.url.startsWith('http:') && !isLocal(auth.url))
    console.warn(
      `smoke: WARNING: sending the token over plain HTTP to ${new URL(auth.url).host}. Use https:// for anything but localhost.`,
    )
  const client = new Client(auth)
  try {
    const res = await fetch(new URL('/health', auth.url), {
      signal: AbortSignal.timeout(TIMEOUT.preflight),
    })
    if (!res.ok) throw new Error(`HTTP ${res.status}`)
  } catch (err) {
    throw new Failure('preflight', `Server not reachable at ${auth.url}: ${(err as Error).message}`)
  }
  let passed = 0
  let last: Run | undefined
  for (let i = 1; i <= args.runs; i++) {
    const started = Date.now()
    last = await runOnce(client, args)
    passed++
    console.log(
      `smoke: run ${i}/${args.runs} ok in ${((Date.now() - started) / 1000).toFixed(1)} s — asked ${[...new Set(last.agents)].join(', ')}, one assistant row, no client assistant POST`,
    )
  }
  if (args.record && last) {
    const fixture = {
      recorded: new Date().toISOString().slice(0, 10),
      server: 'nasiko-cloud-rs cb3aaf0c (local stack)',
      prompt: args.prompt,
      expectedAgent: args.agent,
      frames: scrub(last.frames, { sessionId: last.sessionId, traceId: last.traceId }),
    }
    const text = `${JSON.stringify(fixture, null, 2)}\n`
    if (SECRET.test(text) || text.includes(auth.token))
      throw new Failure(
        'record',
        'refusing to write a fixture that looks like it contains a credential',
      )
    writeFileSync(FIXTURE, text)
    console.log(`smoke: wrote ${FIXTURE}`)
  }
  console.log(`smoke: ${passed}/${args.runs} passed`)
}

// Run only from the CLI: the fixture test imports `scrub` and `KEEP`.
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((err) => {
    const stage = err instanceof Failure ? err.stage : 'unexpected'
    console.error(`smoke: FAILED at ${stage}: ${(err as Error).message}`)
    process.exit(1)
  })
}
