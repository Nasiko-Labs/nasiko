/**
 * Deterministic TokenOps seed (plan D4, A19, A26).
 *
 * One generator feeds the MSW mocks (browser + Vitest) and the live SQL seed
 * (scripts/seed-trace-usage.ts). Numbers come from a fixed PRNG; dates are anchored
 * to `anchor` (default: now) so "This month" always has data, while `--anchor`
 * makes QA runs reproducible.
 *
 * Shaped to mirror the real server's quirks:
 * - agents have a raw `name` (what trace_usage stores) and a different `display_name`
 *   (what the dashboard returns) — exercises the A12 identity join
 * - one agent is deleted: its traces still count in timeseries/calendar but not in
 *   dashboard KPIs (A13 reconciliation gap)
 * - one model has no price → unpriced calls (tokens > 0, cost 0)
 * - one spike day where a single agent's volume jumps ~8x
 * - heavy-tailed volume across agents
 *
 * No imports and erasable-only TypeScript: Node runs this file directly
 * (`node scripts/seed-trace-usage.ts`).
 */

export interface SeedAgent {
  id: string
  /** Raw agent name, as stored in trace_usage.agent_name. */
  name: string
  display_name: string
  deleted: boolean
  version: string
}

interface SeedWorkflow {
  maf_id: string
  workflow_name: string
}

export interface SeedTrace {
  trace_id: string
  session_id: string
  agent_id: string
  /** Raw agent name. */
  agent_name: string
  model: string
  provider: string
  input_tokens: number
  output_tokens: number
  cache_read_tokens: number
  cache_creation_tokens: number
  cost_usd: number
  prompt_cost_usd: number
  completion_cost_usd: number
  latency_ms: number
  tool_call_count: number
  started_at: string
  /** `started_at` as epoch ms, precomputed so aggregations don't re-parse dates. */
  ts: number
  workflow_id: string | null
}

interface SeedInstanceSession {
  agent_id: string
  agent_name: string
  instance_key: string
  started_at: string
  ended_at: string | null
}

export interface Seed {
  anchor: string
  agents: SeedAgent[]
  workflows: SeedWorkflow[]
  traces: SeedTrace[]
  sessions: SeedInstanceSession[]
  spikeDate: string
  spikeAgentName: string
}

export interface SeedOptions {
  anchor?: Date
  days?: number
  agents?: number
  /** Days before the anchor for the spike (0 = today). */
  spikeDay?: number
  prngSeed?: number
}

/** USD per 1M tokens. `custom-local` has no price → unpriced calls. */
export const SEED_MODELS: { model: string; provider: string; inPerM: number; outPerM: number }[] = [
  { model: 'gpt-4o', provider: 'openai', inPerM: 2.5, outPerM: 10 },
  { model: 'gpt-4o-mini', provider: 'openai', inPerM: 0.15, outPerM: 0.6 },
  { model: 'claude-sonnet-4', provider: 'anthropic', inPerM: 3, outPerM: 15 },
  { model: 'gemini-2.0-flash', provider: 'google', inPerM: 0.1, outPerM: 0.4 },
  { model: 'custom-local', provider: 'custom', inPerM: 0, outPerM: 0 },
]

const AGENT_NAMES = [
  ['support-bot', 'Support Bot'],
  ['research-agent', 'Research Agent'],
  ['code-reviewer', 'Code Reviewer'],
  ['sql-analyst', 'SQL Analyst'],
  ['triage-router', 'Triage Router'],
  ['doc-writer', 'Doc Writer'],
  ['sales-assistant', 'Sales Assistant'],
  ['invoice-parser', 'Invoice Parser'],
  ['translator', 'Translator'],
  ['summarizer', 'Summarizer'],
  ['onboarding-guide', 'Onboarding Guide'],
  ['security-scanner', 'Security Scanner'],
  ['data-cleaner', 'Data Cleaner'],
  ['meeting-notes', 'Meeting Notes'],
  ['hr-helpdesk', 'HR Helpdesk'],
  ['pricing-engine', 'Pricing Engine'],
  ['qa-tester', 'QA Tester'],
  ['legal-reviewer', 'Legal Reviewer'],
  ['growth-analyst', 'Growth Analyst'],
  ['legacy-migrator', 'Legacy Migrator'],
  ['forecast-bot', 'Forecast Bot'],
  ['kb-curator', 'KB Curator'],
]

const WORKFLOWS: SeedWorkflow[] = [
  { maf_id: '5eed0000-0000-4000-8000-00000000f001', workflow_name: 'Ticket resolution' },
  { maf_id: '5eed0000-0000-4000-8000-00000000f002', workflow_name: 'Quarterly report' },
  { maf_id: '5eed0000-0000-4000-8000-00000000f003', workflow_name: 'Contract intake' },
]

export const DAY_MS = 86_400_000

/** mulberry32 — tiny, fast, deterministic. Shared with seed-harness.ts. */
export function prng(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

function hex(rand: () => number, n: number): string {
  let s = ''
  for (let i = 0; i < n; i++) s += Math.floor(rand() * 16).toString(16)
  return s
}

/** Stable, UUID-shaped id; the `5eed` prefix makes seed rows easy to find and delete. */
function seedUuid(kind: number, index: number): string {
  const k = kind.toString(16).padStart(4, '0')
  const i = index.toString(16).padStart(12, '0')
  return `5eed0000-${k}-4000-8000-${i}`
}

export function round6(n: number): number {
  return Math.round(n * 1e6) / 1e6
}

export function utcDate(iso: string | Date): string {
  return (typeof iso === 'string' ? new Date(iso) : iso).toISOString().slice(0, 10)
}

export function generateSeed(opts: SeedOptions = {}): Seed {
  const anchor = opts.anchor ?? new Date()
  const days = opts.days ?? 60
  const agentCount = Math.min(opts.agents ?? 20, AGENT_NAMES.length)
  const spikeDay = opts.spikeDay ?? 9
  const rand = prng(opts.prngSeed ?? 20260925)

  const agents: SeedAgent[] = AGENT_NAMES.slice(0, agentCount).map(([name, display], i) => ({
    id: seedUuid(1, i + 1),
    name: `seed-${name}`,
    display_name: display,
    // The last agent is deleted: traces remain, dashboard no longer lists it.
    deleted: i === agentCount - 1,
    version: `1.${i % 4}.0`,
  }))

  // Heavy-tailed daily volume: a few agents dominate.
  const profiles = agents.map((agent, i) => {
    const heavy = Math.pow(rand(), 2.2)
    return {
      agent,
      perDay: Math.max(1, Math.round(2 + heavy * 40)),
      model: SEED_MODELS[i % SEED_MODELS.length],
      tokenScale: 0.4 + rand() * 2.2,
      toolRate: rand() * 3,
      // Per-agent latency profile so cost-vs-performance has real spread.
      latencyScale: 0.4 + Math.pow(rand(), 1.5) * 3,
    }
  })
  // Spike an agent on an expensive model, so the spike is visible in dollars.
  const spikeProfile =
    profiles.find((p) => p.model.model === 'claude-sonnet-4' && !p.agent.deleted) ?? profiles[1]
  const endMs = anchor.getTime()
  const startDay =
    Date.UTC(anchor.getUTCFullYear(), anchor.getUTCMonth(), anchor.getUTCDate()) -
    (days - 1) * DAY_MS
  const spikeDate = utcDate(
    new Date(
      Date.UTC(anchor.getUTCFullYear(), anchor.getUTCMonth(), anchor.getUTCDate()) -
        spikeDay * DAY_MS,
    ),
  )

  const traces: SeedTrace[] = []
  for (let d = 0; d < days; d++) {
    const dayStart = startDay + d * DAY_MS
    const date = utcDate(new Date(dayStart))
    const weekday = new Date(dayStart).getUTCDay()
    const weekFactor = weekday === 0 || weekday === 6 ? 0.45 : 1
    for (const p of profiles) {
      const spike = date === spikeDate && p === spikeProfile ? 8 : 1
      const n = Math.round(p.perDay * weekFactor * spike * (0.6 + rand() * 0.8))
      let sessionId = ''
      for (let k = 0; k < n; k++) {
        // Business-hours weighted time of day (UTC).
        const hour = Math.min(
          23,
          Math.floor(6 + Math.pow(rand(), 0.8) * 15 + (rand() < 0.15 ? -6 : 0)),
        )
        const ts = dayStart + hour * 3_600_000 + Math.floor(rand() * 3_600_000)
        if (ts > endMs) continue
        // Occasional model switch keeps the model filter interesting.
        const model = rand() < 0.2 ? SEED_MODELS[Math.floor(rand() * 4)] : p.model
        const input = Math.round((800 + rand() * 6000) * p.tokenScale * (spike > 1 ? 1.5 : 1))
        const output = Math.round((150 + rand() * 1500) * p.tokenScale)
        const cacheRead = rand() < 0.4 ? Math.round(input * (0.2 + rand() * 0.5)) : 0
        const cacheCreate =
          model.provider === 'anthropic' && rand() < 0.2 ? Math.round(input * 0.3) : 0
        const promptCost = round6(
          ((input - cacheRead) * model.inPerM + cacheRead * model.inPerM * 0.1) / 1e6,
        )
        const completionCost = round6((output * model.outPerM) / 1e6)
        if (k % 3 === 0) sessionId = `5eed-sess-${hex(rand, 12)}`
        traces.push({
          trace_id: `5eed${hex(rand, 28)}`,
          session_id: sessionId,
          agent_id: p.agent.id,
          agent_name: p.agent.name,
          model: model.model,
          provider: model.provider,
          input_tokens: input,
          output_tokens: output,
          cache_read_tokens: cacheRead,
          cache_creation_tokens: cacheCreate,
          cost_usd: round6(promptCost + completionCost),
          prompt_cost_usd: promptCost,
          completion_cost_usd: completionCost,
          latency_ms: Math.round((300 + Math.exp(rand() * 2.4) * 220) * p.latencyScale),
          tool_call_count: Math.floor(rand() * p.toolRate * 2),
          started_at: new Date(ts).toISOString(),
          ts,
          workflow_id:
            rand() < 0.15 ? WORKFLOWS[Math.floor(rand() * WORKFLOWS.length)].maf_id : null,
        })
      }
    }
  }
  traces.sort((a, b) => a.started_at.localeCompare(b.started_at))

  // Container sessions: most agents run continuously with one restart; a few idle.
  const sessions: SeedInstanceSession[] = []
  agents.forEach((agent, i) => {
    const start = startDay + Math.floor(rand() * 5) * DAY_MS
    const restart = start + Math.floor((days / 2) * DAY_MS)
    const replicas = i % 5 === 0 ? 2 : 1
    for (let r = 0; r < replicas; r++) {
      sessions.push({
        agent_id: agent.id,
        agent_name: agent.name,
        instance_key: `seed-${agent.name}-${r}-a`,
        started_at: new Date(start).toISOString(),
        ended_at: new Date(restart).toISOString(),
      })
      sessions.push({
        agent_id: agent.id,
        agent_name: agent.name,
        instance_key: `seed-${agent.name}-${r}-b`,
        started_at: new Date(restart + 600_000).toISOString(),
        ended_at: agent.deleted ? new Date(endMs - 3 * DAY_MS).toISOString() : null,
      })
    }
  })

  return {
    anchor: anchor.toISOString(),
    agents,
    workflows: WORKFLOWS,
    traces,
    sessions,
    spikeDate,
    spikeAgentName: spikeProfile.agent.name,
  }
}
