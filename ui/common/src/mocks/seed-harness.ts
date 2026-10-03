/**
 * Deterministic seed for the Harnesses page (plan §7, T1). It is separate from seed.ts on
 * purpose: its own PRNG, so Harnesses numbers don't move when seed.ts changes. The seed admin's harness turns do
 * reach TokenOps, in mock mode (aggregate.ts withHarnessTurns) and live (`seed:live` writes them into trace_usage),
 * except on seed.ts's spike day, so that day's TokenOps total still equals its chat sessions (follow the money).
 *
 * Shape (every edge case the page must handle is in here):
 * - 3 root units ("departments") with teams under them, and one unit nested under a team;
 * - 60 developers; some in two or three units, direct members of a department, unassigned
 *   developers, one inactive user and one service account (both excluded from scope_devs);
 * - 1–3 registered harnesses per developer, a reinstall (2 rows → 1 registration), an
 *   all-deleted registration (activity still counted), a few "not connected" developers and
 *   one unknown harness id;
 * - 60 days of sessions, with Codex adoption growing and Cursor traffic mostly unpriced;
 * - one user per EE landing rule (the EE layer's mock personas).
 *
 * Mock-only: loaded through the mocks boundary, never in a live bundle (N18).
 */
import { DAY_MS, prng } from './seed.ts'

type HarnessRole = 'admin' | 'manager' | 'member'

export interface HUnit {
  id: string
  parent_id: string | null
  name: string
  lead_id: string | null
}
export interface HUser {
  id: string
  username: string
  display_name: string
  email: string
  role: HarnessRole
  is_superuser: boolean
  is_active: boolean
  service_account: boolean
  unit_ids: string[]
}
export interface HAgent {
  id: string
  owner_id: string
  harness: string
  name: string
  display_name: string
  deleted: boolean
  /** Metadata claims to be a harness but the row has no integration id (spoof test, N4). */
  spoofed?: boolean
}
export interface HSession {
  session_id: string
  user_id: string
  agent_id: string
  harness: string
  model: string
  started_at: string
  /** Epoch ms of started_at. */
  ts: number
  date: string
  turns: number
  unpriced_turns: number
  tokens: number
  cost_usd: number
}
export interface HarnessSeed {
  anchor: string
  units: HUnit[]
  users: HUser[]
  agents: HAgent[]
  sessions: HSession[]
}

/** The default mock viewer (`?as=<seed username>` or `configureMocks({ persona })` picks another). */
export const DEFAULT_PERSONA = 'admin'
/** Same id the auth mock has always returned for admin, so other pages keep working. */
export const ADMIN_ID = '5eed0000-0000-4000-8000-00000000a001'

/** Harness id → CLI name slug (nasiko-server coding_agent_name), display name, models. */
export const HARNESS_META: Record<string, { slug: string; name: string; models: string[] }> = {
  claude: {
    slug: 'claude-code',
    name: 'Claude Code',
    models: ['claude-sonnet-4', 'claude-opus-4'],
  },
  codex: { slug: 'codex', name: 'Codex', models: ['gpt-5-codex'] },
  opencode: { slug: 'opencode', name: 'OpenCode', models: ['claude-sonnet-4', 'custom-local'] },
  cursor: { slug: 'cursor', name: 'Cursor', models: ['cursor-small', 'claude-sonnet-4'] },
  windsurf: { slug: 'windsurf', name: 'Windsurf', models: ['swe-1'] },
}
/** USD per 1M tokens (blended). Missing = unpriced. */
const PRICE: Record<string, number> = {
  'claude-sonnet-4': 6,
  'claude-opus-4': 30,
  'gpt-5-codex': 4,
  'swe-1': 2,
}

/** Stable UUID-shaped id; 5eed0002-* never collides with seed.ts ids (5eed0000-*). */
function hid(kind: number, index: number): string {
  return `5eed0002-${kind.toString(16).padStart(4, '0')}-4000-8000-${index.toString(16).padStart(12, '0')}`
}

/** nasiko-server `coding_agent_name`: lowercase, non-alphanumeric runs → "-", + harness slug. */
export function codingAgentName(username: string, harness: string): string {
  const base = username
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
  return `${base}-${HARNESS_META[harness]?.slug ?? harness}`
}

const FIRST = [
  'Priya',
  'Diego',
  'Aiko',
  'Noah',
  'Fatima',
  'Luca',
  'Mei',
  'Kwame',
  'Sofia',
  'Arjun',
  'Elena',
  'Jonas',
  'Amara',
  'Ravi',
  'Chloe',
  'Mateo',
  'Hana',
  'Ibrahim',
  'Zoe',
  'Kenji',
]
const LAST = [
  'Nair',
  'Alvarez',
  'Tanaka',
  'Becker',
  'Haddad',
  'Rossi',
  'Chen',
  'Mensah',
  'Costa',
  'Iyer',
  'Petrov',
]

export function generateHarnessSeed(
  opts: { anchor?: Date; days?: number; prngSeed?: number } = {},
): HarnessSeed {
  const anchor = opts.anchor ?? new Date()
  const days = opts.days ?? 60
  const rand = prng(opts.prngSeed ?? 20260926)
  const pick = <T>(xs: readonly T[]) => xs[Math.floor(rand() * xs.length)]!

  // ── Org tree ────────────────────────────────────────────────────────────────
  let u = 0
  const unit = (name: string, parent: HUnit | null): HUnit => ({
    id: hid(1, ++u),
    parent_id: parent?.id ?? null,
    name,
    lead_id: null,
  })
  const eng = unit('Engineering', null)
  const product = unit('Product', null)
  const ops = unit('Operations', null)
  const platform = unit('Platform', eng)
  const payments = unit('Payments', eng)
  const growth = unit('Growth', eng)
  const design = unit('Design', product)
  const research = unit('Research', product)
  const support = unit('Support', ops)
  const it = unit('IT', ops)
  const itSec = unit('IT Security', it)
  const units = [
    eng,
    product,
    ops,
    platform,
    payments,
    growth,
    design,
    research,
    support,
    it,
    itSec,
  ]
  const teams = [platform, payments, growth, design, research, support, it]

  // ── People ──────────────────────────────────────────────────────────────────
  const users: HUser[] = []
  let n = 0
  const person = (
    username: string,
    display: string,
    role: HarnessRole,
    unitIds: string[],
    extra: Partial<HUser> = {},
  ): HUser => {
    const user: HUser = {
      id: username === 'admin' ? ADMIN_ID : hid(2, ++n),
      username,
      display_name: display,
      email: `${username}@example.com`,
      role,
      is_superuser: false,
      is_active: true,
      service_account: false,
      unit_ids: unitIds,
      ...extra,
    }
    users.push(user)
    return user
  }
  // Personas (landing rules). Leads are set on units below.
  person('root', 'Rae Root', 'member', [platform.id], { is_superuser: true })
  // The bootstrapped OSS/EE admin is a superuser too (the auth mock has always said so).
  person('admin', 'Ada Admin', 'admin', [], { is_superuser: true })
  const maya = person('maya', 'Maya Okafor', 'manager', [eng.id])
  const omar = person('omar', 'Omar Haddad', 'manager', [product.id])
  const lena = person('lena', 'Lena Vogel', 'manager', [ops.id])
  person('tom', 'Tom Reyes', 'manager', [payments.id])
  person('sam', 'Sam Lee', 'member', [platform.id])
  eng.lead_id = maya.id
  product.lead_id = omar.id
  support.lead_id = omar.id // omar leads two disjoint roots: Product and Support
  ops.lead_id = lena.id
  itSec.lead_id = lena.id // lena's IT Security is nested inside Operations → one root
  // 60 developers: one team each, a few in two or three units.
  for (let i = 0; i < 60; i++) {
    const name = `${FIRST[i % FIRST.length]} ${LAST[(i * 7) % LAST.length]}`
    const username =
      name.toLowerCase().replace(' ', '.') +
      (i >= FIRST.length ? String(Math.floor(i / FIRST.length)) : '')
    const home = teams[i % teams.length]!
    const unitIds = [home.id]
    if (i % 13 === 5) unitIds.push(teams[(i + 2) % teams.length]!.id) // two units
    if (i === 18) unitIds.push(teams[(i + 3) % teams.length]!.id, teams[(i + 5) % teams.length]!.id) // three units
    person(username, name, 'member', unitIds)
  }
  // Direct members of Engineering (in no Engineering team), unassigned people, excluded accounts.
  person('dev.direct1', 'Ines Duarte', 'member', [eng.id])
  person('dev.direct2', 'Yusuf Kaya', 'member', [eng.id])
  person('dev.unassigned1', 'Pia Lund', 'member', [])
  person('dev.unassigned2', 'Theo Marsh', 'member', [])
  person('dev.inactive', 'Olga Former', 'member', [platform.id], { is_active: false })
  person('ci-bot', 'CI Bot', 'member', [platform.id], { service_account: true })

  // ── Registrations ───────────────────────────────────────────────────────────
  const agents: HAgent[] = []
  let a = 0
  const register = (user: HUser, harness: string, extra: Partial<HAgent> = {}) => {
    const meta = HARNESS_META[harness]!
    const agent: HAgent = {
      id: hid(3, ++a),
      owner_id: user.id,
      harness,
      name: codingAgentName(user.username, harness),
      display_name: `${meta.name} (${user.email})`,
      deleted: false,
      ...extra,
    }
    agents.push(agent)
    return agent
  }
  const devs = users.filter((x) => !x.service_account)
  devs.forEach((user, i) => {
    if (i % 17 === 9) return // not connected
    if (rand() < 0.82) register(user, 'claude')
    if (rand() < 0.5) register(user, 'cursor')
    if (rand() < 0.36) register(user, 'codex')
    if (rand() < 0.2) register(user, 'opencode')
    if (!agents.some((ag) => ag.owner_id === user.id)) register(user, pick(['claude', 'cursor']))
  })
  const admin = users.find((x) => x.username === 'admin')!
  for (const h of ['claude', 'codex', 'cursor'])
    if (!agents.some((ag) => ag.owner_id === admin.id && ag.harness === h)) register(admin, h)
  // Reinstall: sam has an old deleted claude row plus the live one (one registration).
  const sam = users.find((x) => x.username === 'sam')!
  agents.push({
    ...(agents.find((ag) => ag.owner_id === sam.id && ag.harness === 'claude') ??
      register(sam, 'claude')),
    id: hid(3, ++a),
    deleted: true,
  })
  // All-deleted: one developer's codex rows are all deleted (0 registrations; activity still counts).
  const lapsed = devs[20]!
  agents.push({
    id: hid(3, ++a),
    owner_id: lapsed.id,
    harness: 'codex',
    name: codingAgentName(lapsed.username, 'codex'),
    display_name: `Codex (${lapsed.email})`,
    deleted: true,
  })
  // Unknown harness id (renders as "Other").
  register(devs[31]!, 'windsurf')
  register(devs[44]!, 'windsurf')
  // Spoofed metadata on an ordinary agent the admin owns (the live adapter must ignore it).
  agents.push({
    id: hid(3, ++a),
    owner_id: admin.id,
    harness: '',
    name: 'admin-helper',
    display_name: 'Helper (claims claude)',
    deleted: false,
    spoofed: true,
  })

  // ── Sessions ────────────────────────────────────────────────────────────────
  const sessions: HSession[] = []
  const end = anchor.getTime()
  let s = 0
  // Idle registrations: dormant for the last 30 days.
  const dormant = new Set(agents.filter((_, i) => i % 6 === 2).map((ag) => ag.id))
  for (const agent of agents) {
    if (agent.spoofed || !agent.harness) continue
    const meta = HARNESS_META[agent.harness]!
    for (let d = days - 1; d >= 0; d--) {
      if (dormant.has(agent.id) && d < 30) continue
      // Codex adoption grows over the window; everything else is steady.
      const base =
        agent.harness === 'codex'
          ? 0.15 + 0.45 * (1 - d / days)
          : agent.harness === 'claude'
            ? 0.55
            : 0.35
      if (agent.deleted && d < 20 && !(agent.owner_id === lapsed.id)) continue // an old reinstall row stops early
      if (rand() > base) continue
      const count = 1 + Math.floor(rand() * 3)
      for (let k = 0; k < count; k++) {
        const ts = end - d * DAY_MS - Math.floor(rand() * 10) * 3_600_000 - k * 900_000
        const model = pick(meta.models)
        const turns = 3 + Math.floor(rand() * 13)
        const tokens = turns * (8_000 + Math.floor(rand() * 20_000))
        // Cursor reports mostly unpriced turns; OpenCode's local model is unpriced.
        const unpricedShare = agent.harness === 'cursor' ? 0.7 : model === 'custom-local' ? 1 : 0
        const unpriced = Math.round(turns * unpricedShare)
        const price = PRICE[model]
        const cost = price
          ? Math.round(((tokens * (turns - unpriced)) / turns / 1e6) * price * 1e6) / 1e6
          : 0
        sessions.push({
          session_id: hid(4, ++s),
          user_id: agent.owner_id,
          agent_id: agent.id,
          harness: agent.harness,
          model,
          started_at: new Date(ts).toISOString(),
          ts,
          date: new Date(ts).toISOString().slice(0, 10),
          turns,
          unpriced_turns: unpriced,
          tokens,
          cost_usd: cost,
        })
      }
    }
  }
  sessions.sort((x, y) => y.ts - x.ts)
  return { anchor: anchor.toISOString(), units, users, agents, sessions }
}

// ── Harness turns in trace_usage (plans/feat-live-contract.md §6-§7) ─────────
// One implementation for both writers: scripts/seed-trace-usage.ts writes these rows into the live trace_usage, and
// the finops mock (handlers.ts) adds them to TokenOps, as the server counts them there.

/** The integrations agents.coding_agent_integration_id's CHECK allows (nasiko-cloud-rs migration 0019). */
const SERVER_HARNESSES = new Set(['claude', 'codex', 'opencode', 'cursor'])
/** A real provider name per model, so seeded turns read like real ones in the provider breakdown. */
export const providerOfModel = (model: string) =>
  model.startsWith('claude') ? 'anthropic' : model.startsWith('gpt') ? 'openai' : 'other'

/** The seed admin's registered harness agents the live seed writes: live, not spoofed, and an integration the server allows. */
export function adminHarnessAgents(hs: HarnessSeed): HAgent[] {
  return hs.agents.filter(
    (a) => a.owner_id === ADMIN_ID && !a.deleted && !a.spoofed && SERVER_HARNESSES.has(a.harness),
  )
}

/** A session's tokens split evenly over its turns; the last turn takes the remainder. */
export function turnTokens(x: { tokens: number; turns: number }, t: number): number {
  const tok = Math.floor(x.tokens / x.turns)
  return t === x.turns - 1 ? x.tokens - tok * (x.turns - 1) : tok
}

export interface HarnessTurn {
  trace_id: string
  session_id: string
  agent_id: string
  model: string
  provider: string
  input_tokens: number
  output_tokens: number
  cost_usd: number
  started_at: string
  ts: number
}

/**
 * One trace_usage row per TURN (the dashboard's `operations` is COUNT(*) of rows, observability/service.rs): the first
 * `unpriced_turns` turns carry no cost and the priced ones share the session's cost; each turn starts a minute after
 * the last. Latency 1200 ms and one tool call are the writers' constants. Turns on `skipDay` (UTC `YYYY-MM-DD`, seed.ts's
 * spike day) are left out, and a session with none left is dropped.
 */
export function harnessTurns(
  hs: HarnessSeed,
  agents: readonly HAgent[] = adminHarnessAgents(hs),
  skipDay?: string,
): HarnessTurn[] {
  const ids = new Set(agents.map((a) => a.id))
  return hs.sessions
    .filter((x) => ids.has(x.agent_id))
    .flatMap((x): HarnessTurn[] => {
      const priced = x.turns - x.unpriced_turns
      return Array.from({ length: x.turns }, (_, t) => {
        const tokens = turnTokens(x, t)
        const ts = Date.parse(x.started_at) + t * 60_000
        return {
          trace_id: `${x.session_id}-${String(t).padStart(2, '0')}`,
          session_id: x.session_id,
          agent_id: x.agent_id,
          model: x.model,
          provider: providerOfModel(x.model),
          input_tokens: Math.round(tokens * 0.8),
          output_tokens: tokens - Math.round(tokens * 0.8),
          cost_usd: t < x.unpriced_turns || !priced ? 0 : x.cost_usd / priced,
          started_at: new Date(ts).toISOString(),
          ts,
        }
      }).filter((t) => !skipDay || !t.started_at.startsWith(skipDay))
    })
}
