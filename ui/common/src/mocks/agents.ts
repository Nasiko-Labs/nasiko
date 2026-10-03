/**
 * Agents mock state (plan §8): derived from the TokenOps seed (A2A agents) and the harness
 * seed (coding harnesses), then mutated in memory by the lifecycle, roll back, secrets,
 * grants and settings handlers so the UI round-trips. Shapes mirror nasiko-server
 * @ cb3aaf0c (catalog/routes.rs, agents/grants.rs, agent_secrets.rs, deployments.rs).
 *
 * Deterministic by index: seed agent 2 crashed and 5 failed (OSS writes no crash fields; the EE layer's mocks add
 * them for agent 2, `CRASHED_AGENT_INDEX`), 7 deploying, 9 stopped, 11 registered, the rest running; 13-16 owned by
 * other seed users. The last seed agent is deleted (seed.ts) and never listed.
 */
import type { components } from '@/lib/api/schema.gen'
import type { LogLine } from '@/features/observability/types'
import type { Seed } from './seed'
import { ADMIN_ID, type HarnessSeed, type HUser } from './seed-harness'

type S = components['schemas']

/** The seed agent that crashed (`status: 'crashed'`, a crashed deployment, restart_count 3). */
export const CRASHED_AGENT_INDEX = 2

export interface MockAgent {
  id: string
  name: string
  display_name: string
  description: string
  owner_id: string
  status: string
  version: string
  tags: string[]
  skills: S['Skill'][]
  capabilities: Record<string, boolean>
  created_at: string
  updated_at: string
  image: string | null
  icon_url: string | null
  documentation_url: string | null
  is_public: boolean
  harness: string | null
  deleted: boolean
  versions: S['AgentVersion'][]
  deployment: S['DeploymentRow'] | null
  secrets: { name: string; updated_at: string | null }[]
  userGrants: string[]
  agentAcl: string[]
  /** Set by roll back: status reads Deploying for ROLLBACK_MS, then the target goes live. */
  rollback: { at: number; to: string; build: string } | null
  /** Written by `PUT /api/agents/{id}`; absent reads as the column defaults (`metadataOf`, false, false). */
  metadata?: Record<string, unknown>
  compress_enabled?: boolean
  compact_tools_enabled?: boolean
  minimal_code_enabled?: boolean
}

export interface AgentsState {
  agents: MockAgent[]
  users: HUser[]
}

/** The worker claims the queued build after this (the old version still reads running), then deploys for ROLLBACK_MS. */
const ROLLBACK_QUEUE_MS = 2_000
const ROLLBACK_MS = 15_000

const SKILLS: Record<string, [string, string, string][]> = {
  default: [
    ['answer', 'Answer questions', 'What can you help me with?'],
    ['summarize', 'Summarize a document', 'Summarize this report in three bullets'],
  ],
  'support-bot': [
    ['triage', 'Triage a ticket', 'My invoice shows the wrong VAT number'],
    ['refund', 'Explain refund policy', 'Can I get a refund after 30 days?'],
    ['escalate', 'Escalate to a human', 'I want to talk to someone'],
  ],
  'sql-analyst': [
    ['query', 'Write a SQL query', 'Top 10 customers by revenue last quarter'],
    ['explain', 'Explain a query plan', 'Why is this join slow?'],
  ],
  'invoice-parser': [
    ['extract', 'Extract invoice fields', 'Parse invoice INV-2026-0142'],
    ['match', 'Match to a purchase order', 'Match INV-2026-0142 to its PO'],
  ],
  // Each agent's own prompts, so a selected agent's chips look like that agent (user review, 2026-09-28). Research
  // Agent keeps a generic first prompt, which the Orchestrator's chips skip.
  'research-agent': [
    ['answer', 'Answer questions', 'What can you help me with?'],
    ['brief', 'Write a research brief', 'Compare three vector databases for our search'],
  ],
  'code-reviewer': [
    ['review', 'Review code in a pull request', 'Review PR #481 for race conditions'],
    ['explain', 'Explain a diff', 'What does this migration change?'],
  ],
  'triage-router': [['route', 'Route a request', 'Who should handle a failed card payment?']],
  'doc-writer': [
    ['draft', 'Draft docs', 'Write a README for the billing service'],
    ['edit', 'Tighten prose', 'Shorten this release note to three lines'],
  ],
  'sales-assistant': [
    ['prep', 'Prep a call', 'Prep me for the Acme renewal call'],
    ['email', 'Draft a follow-up', 'Draft a follow-up after today’s demo'],
  ],
  translator: [['translate', 'Translate text', 'Translate this error message into German']],
  summarizer: [['summarize', 'Summarize a document', 'Summarize this report in three bullets']],
  'onboarding-guide': [
    ['plan', 'Plan onboarding', 'Build a first-week plan for a new support engineer'],
  ],
  'security-scanner': [
    ['scan', 'Scan dependencies', 'Scan the billing service for vulnerable dependencies'],
  ],
  'data-cleaner': [
    ['dedupe', 'Deduplicate records', 'Find duplicate customers in the CRM export'],
    ['normalize', 'Normalize fields', 'Normalize phone numbers to E.164'],
  ],
  'meeting-notes': [['notes', 'Write meeting notes', 'Turn this transcript into action items']],
  'hr-helpdesk': [
    ['leave', 'Answer leave questions', 'How many vacation days do I have left?'],
    ['policy', 'Explain a policy', 'What is the parental leave policy?'],
  ],
  'pricing-engine': [
    ['quote', 'Price a quote', 'Quote 50 seats on the annual plan'],
    ['discount', 'Check a discount', 'Is a 20% discount within policy?'],
  ],
  'qa-tester': [
    ['plan', 'Write a test plan', 'Write test cases for the new checkout flow'],
    ['smoke', 'Run a smoke test', 'Run the smoke suite on staging'],
  ],
  'legal-reviewer': [['review', 'Review a contract', 'Flag risky clauses in this NDA']],
  'growth-analyst': [['funnel', 'Analyse a funnel', 'Where do signups drop off this month?']],
  'legacy-migrator': [['plan', 'Plan a migration', 'Plan moving the cron jobs to the queue']],
  'forecast-bot': [['forecast', 'Forecast a metric', 'Forecast next month’s support volume']],
  'kb-curator': [['gap', 'Find gaps', 'Which help articles are out of date?']],
}

const pad = (n: number) => String(n).padStart(12, '0')
const iso = (ms: number) => new Date(ms).toISOString()

function statusFor(i: number): string {
  return (
    (
      { 2: 'crashed', 5: 'failed', 7: 'deploying', 9: 'stopped', 11: 'registered' } as Record<
        number,
        string
      >
    )[i] ?? 'running'
  )
}

export function buildAgentsState(seed: Seed, hs: HarnessSeed, now: number): AgentsState {
  const others = hs.users.filter((u) => u.id !== ADMIN_ID && !u.service_account).slice(0, 4)
  const day = 86_400_000
  const agents: MockAgent[] = seed.agents.map((a, i) => {
    const key = a.name.replace(/^seed-/, '')
    const created = now - (40 - i) * day
    const status = statusFor(i)
    const vCount = 2 + (i % 3)
    const versions: S['AgentVersion'][] = Array.from(
      { length: status === 'registered' ? 0 : vCount },
      (_, v) => ({
        id: `5eed0003-0000-4000-8000-${pad(i * 10 + v)}`,
        agent_id: a.id,
        build_id: `5eed0004-0000-4000-8000-${pad(i * 10 + v)}`,
        version: `1.${v}.0`,
        image_tag: `registry.local/${key}:1.${v}.0`,
        changelog: v === 0 ? 'Initial release' : `Update ${v}: prompt and tool fixes`,
        is_active: v === vCount - 1,
        can_rollback: v !== vCount - 1,
        previous_version: v ? `1.${v - 1}.0` : null,
        status: v === vCount - 1 ? 'active' : 'archived',
        created_at: iso(created + v * 5 * day),
      }),
    )
    const active = versions.find((v) => v.is_active)
    const deployed = status !== 'registered' && status !== 'stopped'
    return {
      id: a.id,
      name: a.name,
      display_name: a.display_name,
      description: `${a.display_name} handles ${key.replace(/-/g, ' ')} requests over A2A.`,
      owner_id:
        i >= 13 && i <= 16 && others.length ? others[(i - 13) % others.length]!.id : ADMIN_ID,
      status,
      version: active?.version ?? '0.1.0',
      tags: i % 3 === 0 ? ['support', 'customer'] : i % 3 === 1 ? ['data', 'analytics'] : ['ops'],
      skills: (SKILLS[key] ?? SKILLS.default!).map(([id, name, ex]) => ({
        id,
        name,
        description: `${name}.`,
        tags: [],
        examples: [ex],
      })),
      capabilities: {
        streaming: i % 2 === 0,
        pushNotifications: false,
        stateTransitionHistory: i % 4 === 0,
      },
      created_at: iso(created),
      updated_at: iso(now - (i + 1) * 3_600_000),
      image: active ? active.image_tag : null,
      icon_url: null,
      documentation_url: i === 0 ? 'https://example.com/docs/support-bot' : null,
      is_public: i % 4 === 0,
      harness: null,
      deleted: a.deleted,
      versions,
      deployment: deployed
        ? {
            id: `5eed0005-0000-4000-8000-${pad(i)}`,
            agent_id: a.id,
            agent_name: a.name,
            build_id: active?.build_id ?? `5eed0004-0000-4000-8000-${pad(i * 10)}`,
            namespace: 'default',
            replicas: 1,
            status:
              status === 'crashed' || status === 'failed'
                ? 'crashed'
                : status === 'deploying'
                  ? 'starting'
                  : 'running',
            service_url: `http://localhost:${9100 + i}`,
            owner_id: ADMIN_ID,
            created_at: iso(now - 2 * day),
            restart_count: status === 'crashed' ? 3 : 0,
            // Crash guardian fields: OSS never writes them (the EE layer's mocks do, for CRASHED_AGENT_INDEX).
            crash_reason: null,
            crashed_at: null,
            last_logs: null,
            k8s_deployment_name: null,
          }
        : null,
      secrets: i % 2 === 0 ? [{ name: 'OPENAI_API_KEY', updated_at: null }] : [],
      userGrants: i === 0 && others[0] ? [others[0].id] : [],
      agentAcl: [],
      rollback: null,
    }
  })
  for (const h of hs.agents) {
    agents.push({
      id: h.id,
      name: h.name,
      display_name: h.display_name,
      description: '',
      owner_id: h.owner_id,
      status: 'registered',
      version: '1.0.0',
      tags: ['local', 'coding-agent'],
      skills: [],
      capabilities: {},
      created_at: hs.anchor,
      updated_at: hs.anchor,
      image: null,
      icon_url: null,
      documentation_url: null,
      is_public: false,
      harness: h.spoofed ? null : h.harness,
      deleted: h.deleted,
      versions: [],
      deployment: null,
      secrets: [],
      userGrants: [],
      agentAcl: [],
      rollback: null,
    })
  }
  return { agents, users: hs.users.filter((u) => !u.service_account) }
}

/** Current status, advancing a roll back once its time has passed. */
export function liveStatus(a: MockAgent, now: number): string {
  if (!a.rollback) return a.status
  // agents/update.rs: rollback only queues a build_jobs row; `deploying` is written when the worker claims it.
  if (now - a.rollback.at < ROLLBACK_QUEUE_MS) return a.status
  if (now - a.rollback.at < ROLLBACK_QUEUE_MS + ROLLBACK_MS) return 'deploying'
  const to = a.rollback.to
  a.versions = a.versions.map((v) => ({
    ...v,
    is_active: v.version === to,
    can_rollback: v.version !== to,
    status: v.version === to ? 'active' : 'archived',
  }))
  a.version = to
  a.status = 'running'
  a.rollback = null
  return a.status
}

/** catalog/routes.rs at ea233d20 always sends all four A2A capability flags, defaulting to false. */
const capabilities = (a: MockAgent) => ({
  chat_agent: false,
  pushNotifications: false,
  stateTransitionHistory: false,
  streaming: false,
  ...a.capabilities,
})

export function listRow(a: MockAgent, now: number): S['Agent'] {
  return {
    id: a.id,
    name: a.name,
    display_name: a.display_name,
    description: a.description,
    owner_id: a.owner_id,
    status: liveStatus(a, now),
    version: a.version,
    created_at: a.created_at,
    updated_at: a.updated_at,
    capabilities: capabilities(a),
    url: null,
    transport_path: null,
    metadata: metadataOf(a),
    security_schemes: {},
    default_input_modes: ['text'],
    default_output_modes: ['text'],
    preferred_transport: 'JSONRPC',
    protocol_version: '0.3.0',
    skills: a.skills,
    tags: a.tags,
    image: a.image,
    icon_url: a.icon_url,
    documentation_url: a.documentation_url,
  } satisfies S['Agent']
}

/** `AgentDetailResponse`: camelCase except the renamed fields (catalog/routes.rs:616). Status NOT reconciled. */
function metadataOf(a: MockAgent): Record<string, unknown> {
  if (a.metadata) return a.metadata
  return a.harness !== null || a.tags.includes('coding-agent')
    ? { source: 'nasiko-cli-integration', integration_id: a.harness ?? 'claude' }
    : {}
}

/** nasiko-coding-policy `CODING_TERMS`: a word of a skill's id, name or tag starts with one (models.rs `has_coding_skills`). */
const CODING = /^(cod|program|software|refactor|debug|bug|lint|compil)/i
const mentionsCoding = (text: string) => text.split(/[^a-z0-9]+/i).some((w) => CODING.test(w))

export function detailBody(a: MockAgent, canManage: boolean, compactToolsAvailable = true) {
  return {
    id: a.id,
    name: a.name,
    display_name: a.display_name,
    description: a.description,
    owner_id: a.owner_id,
    // get_one copies agents.status as stored; a roll back in flight isn't visible here.
    status: a.status,
    version: a.version,
    url: `/api/agents/${a.id}`,
    iconUrl: a.icon_url,
    documentationUrl: a.documentation_url,
    protocolVersion: '0.3.0',
    preferredTransport: 'JSONRPC',
    defaultInputModes: ['text'],
    defaultOutputModes: ['text'],
    capabilities: capabilities(a),
    skills: a.skills,
    tags: a.tags,
    additionalInterfaces: null,
    securitySchemes: {},
    security: [],
    signatures: [],
    supportsAuthenticatedExtendedCard: false,
    provider: null,
    can_manage: canManage,
    is_coding_agent: a.harness !== null,
    coding_agent_integration_id: a.harness,
    compress_enabled: a.compress_enabled ?? false,
    compact_tools_enabled: a.compact_tools_enabled ?? false,
    compact_tools_available: compactToolsAvailable,
    minimal_code_enabled: a.minimal_code_enabled ?? false,
    has_coding_skills: a.skills.some((k) =>
      [k.id, k.name, ...(k.tags ?? [])].some((t) => mentionsCoding(t)),
    ),
    metadata: metadataOf(a),
    created_at: a.created_at,
    updated_at: a.updated_at,
  }
}

/** Newest first (observability/routes.rs sorts `Reverse(timestamp)`); crashed agents carry ERROR lines. */
export function logLines(a: MockAgent, now: number, level?: string | null, limit = 200): LogLine[] {
  const base: LogLine[] = Array.from({ length: 60 }, (_, k) => ({
    timestamp: iso(now - (k + 1) * 60_000),
    level: k % 7 === 3 ? 'WARN' : 'INFO',
    message: `${a.name}: ${k % 2 ? 'POST /a2a message/send 200' : 'tool call completed'}`,
    source: k % 3 === 0 ? 'proxy' : 'container',
    trace_id: null,
  }))
  const s = liveStatus(a, now)
  if (s === 'crashed' || s === 'failed') {
    base.splice(1, 0, {
      timestamp: iso(now - 90_000),
      level: 'ERROR',
      message: `${a.name}: worker exited with code 137`,
      source: 'container',
      trace_id: null,
    })
    base.splice(4, 0, {
      timestamp: iso(now - 4 * 60_000),
      level: 'ERROR',
      message: `${a.name}: upstream model call failed: 502`,
      source: 'container',
      trace_id: null,
    })
  }
  // The level filter runs after the limit cut, like the server (routes.rs:277-288).
  const cut = base.sort((x, y) => y.timestamp.localeCompare(x.timestamp)).slice(0, limit)
  return level ? cut.filter((l) => l.level === level.toUpperCase()) : cut
}
