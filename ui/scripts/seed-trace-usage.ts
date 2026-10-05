/**
 * Emit SQL that loads the TokenOps seed into a LOCAL OSS nasiko-server Postgres
 * (plan A26). It writes only database rows; it never touches the nasiko-cloud-rs repo.
 *
 *   node scripts/seed-trace-usage.ts [--anchor 2026-09-25T12:00:00Z] [--days 60]
 *                                    [--agents 20] [--spike-day 9] [--reset] [--marker]
 *
 * Usually run through `npm run seed:live`, which pipes this into the infra Postgres.
 *
 * - Inserts matching `agents` rows first (trace_usage.agent_id has an FK to agents),
 *   owned by the bootstrapped superuser; one agent is soft-deleted on purpose.
 * - Inserts `trace_usage` (finops reads this) and `agent_instance_sessions`
 *   (container hours for F5).
 * - Idempotent across days: one transaction that first removes previous seed rows, then
 *   inserts the new set, so re-running re-anchors the data instead of piling up sessions.
 * - `--reset` only removes seed rows (agents with ids 5eed0000-0001-*, their traces and
 *   instance sessions); real rows are never matched.
 * - Seeded data is visible to the superuser (admin); other users only see agents they
 *   can access, which these are not.
 * - Workflows (maf_executions) are not seeded; the Workflows view shows real data only.
 * - Harnesses (plan §8): coding-agent `agents` rows (ids 5eed0002-*) for the superuser, one
 *   per harness the seed admin uses, plus their `trace_usage` and `chat_sessions` rows, so the
 *   Harnesses live fallback shows data without Tempo. Agent names are computed IN SQL from the
 *   superuser's username (nasiko-server coding_agent_name), and every insert is guarded
 *   `WHERE NOT EXISTS`, so a real harness the developer already registered is skipped, never
 *   an error that rolls back the whole seed.
 * - LLM router (plans/feat-llm-router.md §6): `llm_configs` (ids 5eed0007-*) created by the superuser, routing on
 *   three seed agents, and `token_usage` rows (ids 5eed000a-*) so the router page's spend column has data. `--reset`
 *   unsets any routing that points at a seed config before deleting the configs.
 * - MCP servers (plans/feat-mcp.md §8): the `live` rows of `common/src/mocks/seed-mcp.ts` (ids 5eed000e-*, builds
 *   5eed000f-*), owned by the superuser: platform toolkits, registered and uploaded servers with their tools, the
 *   superuser's credential-free connections, and agent access on the first seed agents. A server whose name the
 *   superuser (or the platform) already uses is skipped, and its rows with it, never an error that rolls back the seed.
 * - Live contract (plans/feat-live-contract.md §6): `--marker` adds the seed marker, an llm_config named
 *   `seed-marker-<ISO anchor>` (id 5eed0007-…-000000000099), which the recorder reads back; only for the throwaway
 *   contract and EE databases (scripts/seed-live.ts decides). The seed then asserts no seed agent has an `image` or
 *   `status = 'running'`, so a contract server never tries to deploy one.
 */
import { parseArgs } from 'node:util'
import { generateSeed } from '../common/src/mocks/seed.ts'
import {
  adminHarnessAgents,
  generateHarnessSeed,
  HARNESS_META,
  harnessTurns,
  turnTokens,
} from '../common/src/mocks/seed-harness.ts'
import {
  MCP_ACCESS,
  MCP_CONNECTIONS,
  MCP_CONNECTORS,
  mcpBuildId,
  mcpConnectorId,
} from '../common/src/mocks/seed-mcp.ts'
import { seedTraceSessions, TRACE_WINDOW_DAYS } from './lib/otlp.ts'

function parseFlags() {
  try {
    return parseArgs({
      options: {
        anchor: { type: 'string' },
        days: { type: 'string' },
        agents: { type: 'string' },
        'spike-day': { type: 'string' },
        reset: { type: 'boolean', default: false },
        marker: { type: 'boolean', default: false },
      },
    }).values
  } catch (err) {
    // Unknown flags or a value that looks like a flag (`--spike-day -1`): a message and
    // exit 2, like the other validation errors, not a stack trace.
    process.stderr.write(`seed-trace-usage: ${(err as Error).message}\n`)
    process.exit(2)
  }
}
const values = parseFlags()

function positiveInt(name: string, raw: string | undefined, min = 1): number | undefined {
  if (raw === undefined) return undefined
  const n = Number(raw)
  if (!Number.isInteger(n) || n < min) {
    process.stderr.write(`seed-trace-usage: --${name} must be an integer >= ${min}, got "${raw}"\n`)
    process.exit(2)
  }
  return n
}

const q = (v: string | null) => (v === null ? 'NULL' : `'${v.replace(/'/g, "''")}'`)

const SUPERUSER =
  '(SELECT id FROM users WHERE is_superuser AND deleted_at IS NULL ORDER BY created_at LIMIT 1)'
/** The seed marker (plans/feat-live-contract.md §6): its name carries the exact anchor the seed used. */
const MARKER_ID = '5eed0007-0000-4000-8000-000000000099'
const MARKER_PREFIX = 'seed-marker-'
/** nasiko-server coding_agent_name(): lowercase, runs of non-alphanumerics → "-", edges trimmed. */
const nameExpr = (slug: string) =>
  `btrim(regexp_replace(lower(u.username), '[^a-z0-9]+', '-', 'g'), '-') || '-${slug}'`

/**
 * A seeded harness agent the CLI has adopted: registration (catalog/routes.rs) reuses an existing
 * row with the same owner + name + integration, so after `nasiko agents install` the seed row IS
 * the developer's real agent. Real use leaves a gateway token or non-seed usage rows; such an agent
 * is never deleted or given seed traffic again.
 */
const ADOPTED = (
  a: string,
) => `(EXISTS (SELECT 1 FROM agent_gateway_tokens t WHERE t.agent_id = ${a}.id)
    OR EXISTS (SELECT 1 FROM trace_usage t WHERE t.agent_id = ${a}.id AND t.trace_id NOT LIKE '5eed0002%')
    OR EXISTS (SELECT 1 FROM chat_sessions c WHERE c.agent_id = ${a}.id AND c.session_id NOT LIKE '5eed0002%')
    OR EXISTS (SELECT 1 FROM coding_agent_telemetry_events e WHERE e.agent_id = ${a}.id)
    OR EXISTS (SELECT 1 FROM mcp_agent_connector_access c WHERE c.agent_id = ${a}.id)
    OR EXISTS (SELECT 1 FROM agent_grants g WHERE g.agent_id = ${a}.id)
    OR EXISTS (SELECT 1 FROM agent_acl l WHERE l.caller_agent_id = ${a}.id OR l.target_agent_id = ${a}.id)
    OR ${a}.llm_config_id IS NOT NULL OR ${a}.pinned_model IS NOT NULL OR ${a}.is_public)`
// (The last lines catch an installed-and-configured agent before any traffic: its connector
// access, grants, ACL rows and settings would otherwise cascade away with it.)
/** Seeded chat_messages carry this trace_id, so a message someone really sent is told apart. */
const SEED_MESSAGE_TRACE = '5eed0002-message'
// A seed session someone kept working in is kept on reset: deleting it would cascade into their
// real chat_messages, and into real telemetry (coding_agent_telemetry_events.session_id is
// ON DELETE CASCADE since migration 0021).
const SEED_SESSION_IN_USE = `(EXISTS (SELECT 1 FROM coding_agent_telemetry_events e WHERE e.session_id = chat_sessions.session_id)
    OR EXISTS (SELECT 1 FROM chat_messages m WHERE m.session_id = chat_sessions.session_id AND m.trace_id IS DISTINCT FROM '${SEED_MESSAGE_TRACE}')
    OR EXISTS (SELECT 1 FROM hitl_requests h WHERE h.chat_session_id = chat_sessions.session_id))`
const SEED_AGENT_FILTER = "agent_id::text LIKE '5eed0000-0001-%'"
// trace_usage.agent_id is ON DELETE SET NULL: a hard-deleted seed agent leaves its traces
// orphaned with the seed name, and the fixed trace ids would then collide on the next run.
const DELETE_SEED = [
  // Harness rows first (trace_usage/chat_sessions reference the agents).
  "DELETE FROM trace_usage WHERE trace_id LIKE '5eed0002%' AND (agent_id IS NULL OR agent_id::text LIKE '5eed0002-%');",
  `DELETE FROM chat_sessions WHERE session_id LIKE '5eed0002%' AND NOT ${SEED_SESSION_IN_USE};`,
  `SELECT 'kept adopted harness agent ' || a.name || ' (' || a.id || ')' AS notice FROM agents a WHERE a.id::text LIKE '5eed0002-%' AND ${ADOPTED('a')};`,
  `DELETE FROM agents a WHERE a.id::text LIKE '5eed0002-%' AND NOT ${ADOPTED('a')};`,
  // The contract seed's chat sessions (the observability mock's, `5eed-sess-*`) reference seed agents.
  `DELETE FROM chat_sessions WHERE session_id LIKE '5eed-sess-%' AND NOT ${SEED_SESSION_IN_USE};`,
  `DELETE FROM trace_usage WHERE trace_id LIKE '5eed%' AND (${SEED_AGENT_FILTER} OR (agent_id IS NULL AND agent_name LIKE 'seed-%'));`,
  `DELETE FROM agent_instance_sessions WHERE instance_key LIKE 'seed-%' AND ${SEED_AGENT_FILTER};`,
  "DELETE FROM agents WHERE id::text LIKE '5eed0000-0001-%';",
  // LLM router (plans/feat-llm-router.md §6): usage rows, then routing on any agent still pointing at a seed config
  // (someone may have attached a real agent to one in the UI; the FK would block the delete), then the configs.
  "DELETE FROM token_usage WHERE id::text LIKE '5eed000a-%';",
  "UPDATE agents SET llm_config_id = NULL, pinned_model = NULL WHERE llm_config_id::text LIKE '5eed0007-%';",
  "DELETE FROM llm_configs WHERE id::text LIKE '5eed0007-%';",
  // MCP servers (plans/feat-mcp.md §8): tools, grants, connections, agent access and builds cascade with them.
  "DELETE FROM mcp_connectors WHERE id::text LIKE '5eed000e-%';",
]

const out: string[] = ['BEGIN;']

if (values.reset) {
  out.push(...DELETE_SEED)
} else {
  let anchor: Date | undefined
  if (values.anchor !== undefined) {
    anchor = new Date(values.anchor)
    if (Number.isNaN(anchor.getTime())) {
      process.stderr.write(
        `seed-trace-usage: --anchor must be an ISO date/time, got "${values.anchor}"\n`,
      )
      process.exit(2)
    }
  }
  const seed = generateSeed({
    anchor,
    days: positiveInt('days', values.days),
    agents: positiveInt('agents', values.agents),
    spikeDay: positiveInt('spike-day', values['spike-day'], 0),
  })

  out.push(`DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM users WHERE is_superuser AND deleted_at IS NULL) THEN
    RAISE EXCEPTION 'nasiko-ui-lab seed: no superuser found. Start nasiko-server once (it bootstraps ADMIN_USERNAME), then re-run.';
  END IF;
END $$;`)
  // Replace, don't accumulate: previous runs' rows go first, inside the same transaction.
  out.push(...DELETE_SEED)

  const agentValues = seed.agents
    .map(
      (a) =>
        `(${q(a.id)}::uuid, ${q(a.name)}, ${q(a.display_name)}, ${q(a.version)}, ${a.deleted ? 'now()' : 'NULL'})`,
    )
    .join(',\n  ')
  out.push(`INSERT INTO agents (id, name, display_name, version, deleted_at, owner_id, description)
SELECT v.id, v.name, v.display_name, v.version, v.deleted_at,
       (SELECT id FROM users WHERE is_superuser AND deleted_at IS NULL ORDER BY created_at LIMIT 1),
       'Seeded by nasiko-ui-lab (scripts/seed-trace-usage.ts)'
FROM (VALUES
  ${agentValues}
) AS v(id, name, display_name, version, deleted_at);`)

  out.push(...routerSeed(seed))
  out.push(...mcpSeed(seed))
  if (values.marker) out.push(markerSeed(seed.anchor), ...traceChatSessions(seed))

  const cols =
    'trace_id, agent_name, session_id, agent_id, model, provider, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, cost_usd, prompt_cost_usd, completion_cost_usd, latency_ms, started_at, tool_call_count'
  for (let i = 0; i < seed.traces.length; i += 500) {
    const rows = seed.traces
      .slice(i, i + 500)
      .map(
        (t) =>
          `(${q(t.trace_id)}, ${q(t.agent_name)}, ${q(t.session_id)}, ${q(t.agent_id)}::uuid, ${q(t.model)}, ${q(t.provider)}, ${t.input_tokens}, ${t.output_tokens}, ${t.cache_read_tokens}, ${t.cache_creation_tokens}, ${t.cost_usd}, ${t.prompt_cost_usd}, ${t.completion_cost_usd}, ${t.latency_ms}, ${q(t.started_at)}::timestamptz, ${t.tool_call_count})`,
      )
    out.push(`INSERT INTO trace_usage (${cols}) VALUES\n${rows.join(',\n')};`)
  }

  const sessionRows = seed.sessions.map(
    (s) =>
      // An open session was last seen at the anchor (not the database's now()), so its hours are deterministic and match
      // the mock (agents/hours_meter.rs ends an open session at last_seen_at).
      `(${q(s.agent_id)}::uuid, ${q(s.agent_name)}, ${q(s.instance_key)}, 'docker', ${q(s.started_at)}::timestamptz, ${s.ended_at ? `${q(s.ended_at)}::timestamptz` : `${q(seed.anchor)}::timestamptz`}, ${s.ended_at ? `${q(s.ended_at)}::timestamptz` : 'NULL'})`,
  )
  out.push(
    `INSERT INTO agent_instance_sessions (agent_id, agent_name, instance_key, runtime, started_at, last_seen_at, ended_at) VALUES\n${sessionRows.join(',\n')};`,
  )
  out.push(
    `-- seeded ${seed.agents.length} agents, ${seed.traces.length} traces, ${seed.sessions.length} instance sessions; spike ${seed.spikeDate} on ${seed.spikeAgentName}`,
  )

  // ── Harnesses (plan §8) ──────────────────────────────────────────────────────
  const hs = generateHarnessSeed({
    anchor: seed.agents.length ? new Date(seed.anchor) : undefined,
    days: positiveInt('days', values.days),
  })
  const adminAgents = adminHarnessAgents(hs)
  for (const a of adminAgents) {
    const { slug, name } = HARNESS_META[a.harness]!
    // Skip when the superuser already has a live agent with this name or this integration (both partial unique indexes).
    out.push(`INSERT INTO agents (id, name, display_name, description, owner_id, tags, metadata, coding_agent_integration_id)
SELECT ${q(a.id)}::uuid, ${nameExpr(slug)}, ${q(name)} || ' (' || coalesce(u.email, u.username) || ')',
       'Seeded by nasiko-ui-lab (scripts/seed-trace-usage.ts)', u.id, ARRAY['local','coding-agent'],
       jsonb_build_object('source', 'nasiko-cli-integration', 'integration_id', ${q(a.harness)}), ${q(a.harness)}
FROM users u
WHERE u.id = ${SUPERUSER}
  AND NOT EXISTS (SELECT 1 FROM agents x WHERE x.owner_id = u.id AND x.deleted_at IS NULL
                  AND (x.name = ${nameExpr(slug)} OR x.coding_agent_integration_id = ${q(a.harness)}))
  -- A kept (adopted) seed agent, even a since soft-deleted one, already holds this id.
  AND NOT EXISTS (SELECT 1 FROM agents y WHERE y.id = ${q(a.id)}::uuid);`)
  }
  // One trace_usage row per TURN (seed-harness.ts harnessTurns, shared with the finops mock), none on the spike day.
  const turns = harnessTurns(hs, adminAgents, seed.spikeDate)
  const kept = new Set(turns.map((t) => t.session_id))
  const hsessions = hs.sessions.filter((x) => kept.has(x.session_id))
  const turnRows = turns.map(
    (r) =>
      `(${q(r.trace_id)}, ${q(r.session_id)}, ${q(r.agent_id)}::uuid, ${q(r.model)}, ${q(r.provider)}, ${r.input_tokens}, ${r.output_tokens}, ${r.cost_usd}, ${q(r.started_at)}::timestamptz)`,
  )
  for (let i = 0; i < turnRows.length; i += 400) {
    // Only for seed agents actually inserted above and not adopted by a real registration.
    out.push(`INSERT INTO trace_usage (trace_id, agent_name, session_id, agent_id, user_id, model, provider, input_tokens, output_tokens, cost_usd, prompt_cost_usd, completion_cost_usd, latency_ms, started_at, tool_call_count)
SELECT v.tid, a.name, v.sid, a.id, a.owner_id, v.model, v.provider, v.inp, v.outp, v.cost, v.cost * 0.3, v.cost * 0.7, 1200, v.started, 1
FROM (VALUES
${turnRows.slice(i, i + 400).join(',\n')}
) AS v(tid, sid, agent_id, model, provider, inp, outp, cost, started)
JOIN agents a ON a.id = v.agent_id
WHERE NOT ${ADOPTED('a')};`)
  }
  for (let i = 0; i < hsessions.length; i += 400) {
    // updated_at = the last turn, as a real session's last message would move it.
    const vals = hsessions
      .slice(i, i + 400)
      .map(
        (x) =>
          `(${q(x.session_id)}, ${q(x.agent_id)}::uuid, ${q(x.started_at)}::timestamptz, ${q(new Date(Date.parse(x.started_at) + x.turns * 60_000).toISOString())}::timestamptz)`,
      )
      .join(',\n')
    out.push(`INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title, created_at, updated_at)
SELECT v.sid, a.owner_id, a.id, '/api/agents/' || a.id, a.display_name, v.started, v.updated
FROM (VALUES
${vals}
) AS v(sid, agent_id, started, updated)
JOIN agents a ON a.id = v.agent_id
WHERE NOT ${ADOPTED('a')}
ON CONFLICT (session_id) DO NOTHING;`)
  }
  // chat/routes.rs takes a session's message_count and total_tokens from chat_messages: one user +
  // one assistant message per turn, so the live session list matches the mock. Only for seed
  // sessions that have no messages yet (a kept, in-use session is left alone); they cascade away
  // with their session on reset.
  const messageRows = hsessions.flatMap((x) => {
    return Array.from({ length: x.turns }, (_, t) => {
      const at = Date.parse(x.started_at) + t * 60_000
      const tokens = turnTokens(x, t)
      return [
        `(${q(x.session_id)}, 'user', 'seeded prompt', ${q(new Date(at).toISOString())}::timestamptz, NULL, NULL, NULL)`,
        `(${q(x.session_id)}, 'assistant', 'seeded reply', ${q(new Date(at + 30_000).toISOString())}::timestamptz, ${Math.round(tokens * 0.8)}, ${tokens - Math.round(tokens * 0.8)}, ${q(x.model)})`,
      ]
    }).flat()
  })
  // Batches hold whole sessions: the "no messages yet" guard runs per statement, so a session
  // split across two batches would lose its second half to the first batch's rows.
  const bySession: string[][] = []
  for (const x of hsessions) {
    const rows = messageRows.filter((r) => r.startsWith(`(${q(x.session_id)},`))
    const last = bySession.at(-1)
    if (last && last.length + rows.length <= 400) last.push(...rows)
    else bySession.push([...rows])
  }
  for (const batch of bySession) {
    out.push(`INSERT INTO chat_messages (session_id, role, content, timestamp, input_tokens, output_tokens, model, trace_id)
SELECT v.sid, v.role, v.content, v.at, v.inp::int, v.outp::int, v.model, '${SEED_MESSAGE_TRACE}'
FROM (VALUES
${batch.join(',\n')}
) AS v(sid, role, content, at, inp, outp, model)
JOIN chat_sessions cs ON cs.session_id = v.sid
WHERE v.sid LIKE '5eed0002%' AND NOT EXISTS (SELECT 1 FROM chat_messages m WHERE m.session_id = v.sid);`)
  }
  // A contract server's startup reconciler redeploys agents that have an image and status 'running'
  // (agents/reconcile.rs); seed agents must never qualify. Contract and EE runs only (--marker): a dev database may
  // hold a seed agent someone adopted and deployed on purpose.
  if (values.marker)
    out.push(`DO $$ BEGIN
  IF EXISTS (SELECT 1 FROM agents WHERE id::text LIKE '5eed%' AND (image IS NOT NULL OR status = 'running')) THEN
    RAISE EXCEPTION 'nasiko-ui-lab seed: a seed agent has an image or status running; the server would try to deploy it';
  END IF;
END $$;`)
  out.push(
    `-- harnesses: ${adminAgents.length} coding agents (skipped if the superuser already has them), ${hsessions.length} sessions, ${turnRows.length} turns, ${messageRows.length} messages`,
  )
}

out.push('COMMIT;')
process.stdout.write(`${out.join('\n\n')}\n`)

/**
 * LLM router rows (plans/feat-llm-router.md §6): `llm_configs` (ids 5eed0007-*) created by the superuser who owns the
 * seed agents, routing on three seed agents, and `token_usage` rows (ids 5eed000a-*) for the last 30 days of seed
 * traces, so the router page's spend column has live data. Configs use the platform key, except one that names a
 * secret on purpose missing (the seed can't encrypt secrets). `ON CONFLICT DO NOTHING` skips the default when the
 * superuser already has one (one default per owner) and any name they already use; routing is set only onto configs
 * that exist and belong to the agent's owner. `cost_usd` is written explicitly: the server's SQL pricing trigger was
 * dropped (nasiko-cloud-rs migration 0038), and a NULL cost reads as $0.
 */
function routerSeed(seed: ReturnType<typeof generateSeed>): string[] {
  const cid = (n: number) => `5eed0007-0000-4000-8000-${String(n).padStart(12, '0')}`
  const configs: [
    number,
    string,
    string,
    string,
    string[],
    string | null,
    boolean,
    [string, string, string] | null,
    string | null,
  ][] = [
    [
      1,
      'seed-default',
      'anthropic',
      'claude-sonnet-4',
      ['openai/gpt-4o-mini'],
      null,
      true,
      null,
      null,
    ],
    [
      2,
      'seed-openai-tiers',
      'openai',
      'gpt-4o-mini',
      ['gpt-4o-mini'],
      null,
      false,
      ['gpt-4o', 'gpt-4o-mini', 'gpt-4o-mini'],
      null,
    ],
    [3, 'seed-fast', 'openai', 'gpt-4o-mini', [], null, false, null, 'gpt-4o-mini'],
    [4, 'seed-missing-key', 'openai', 'gpt-4o', [], 'SEED_MISSING_KEY', false, null, null],
  ]
  const cfgRows = configs.map(
    ([n, name, provider, model, fb, secret, def, tiers, pin]) =>
      `(${q(cid(n))}::uuid, ${q(name)}, ${q(provider)}, ${q(model)}, ${q(JSON.stringify(fb))}::jsonb, ${q(secret)}, ${def}, ${q(tiers?.[0] ?? null)}, ${q(tiers?.[1] ?? null)}, ${q(tiers?.[2] ?? null)}, ${pin !== null}, ${q(pin)})`,
  )
  const live = seed.agents.filter((a) => !a.deleted)
  const routing: [string, number, string | null][] = [
    [live[0]?.id, 2, null],
    [live[1]?.id, 2, 'gpt-4o'],
    [live[2]?.id, 3, null],
  ].filter((r): r is [string, number, string | null] => !!r[0])
  const out = [
    `INSERT INTO llm_configs (id, created_by, name, provider, model, fallback_models, api_key_secret_name, is_default, tier1_model, tier2_model, tier3_model, pinned, pinned_model)
SELECT v.id, ${SUPERUSER}, v.name, v.provider, v.model, v.fb, v.secret, v.def, v.t1, v.t2, v.t3, v.pinned, v.pin
FROM (VALUES
${cfgRows.join(',\n')}
) AS v(id, name, provider, model, fb, secret, def, t1, t2, t3, pinned, pin)
ON CONFLICT DO NOTHING;`,
    `UPDATE agents a SET llm_config_id = c.id, pinned_model = v.pin
FROM (VALUES
${routing.map(([id, n, pin]) => `(${q(id)}::uuid, ${q(cid(n))}::uuid, ${q(pin)})`).join(',\n')}
) AS v(agent_id, cfg, pin)
JOIN llm_configs c ON c.id = v.cfg
WHERE a.id = v.agent_id AND a.id::text LIKE '5eed0000-0001-%' AND c.created_by = a.owner_id AND c.deleted_at IS NULL;`,
  ]
  const since = Date.parse(seed.anchor) - 30 * 86_400_000
  const usage = seed.traces.filter((t) => t.agent_id && Date.parse(t.started_at) >= since)
  const rows = usage.map(
    (t, i) =>
      `(${q(`5eed000a-0000-4000-8000-${String(i + 1).padStart(12, '0')}`)}::uuid, ${q(t.agent_id)}::uuid, ${q(t.provider)}, ${q(t.model)}, ${t.input_tokens}, ${t.output_tokens}, ${t.cost_usd}, ${t.latency_ms}, ${q(t.session_id)}, ${q(t.started_at)}::timestamptz)`,
  )
  for (let i = 0; i < rows.length; i += 500) {
    out.push(`INSERT INTO token_usage (id, user_id, agent_id, operation_type, provider, model, input_tokens, output_tokens, total_tokens, cost_usd, latency_ms, session_id, metadata, created_at)
SELECT v.id, ${SUPERUSER}, a.id, 'direct_llm', v.provider, v.model, v.inp, v.outp, v.inp + v.outp, v.cost, v.lat, v.sid, '{"key_source": "platform", "attribution": null}'::jsonb, v.at
FROM (VALUES
${rows.slice(i, i + 500).join(',\n')}
) AS v(id, agent_id, provider, model, inp, outp, cost, lat, sid, at)
JOIN agents a ON a.id = v.agent_id;`)
  }
  out.push(
    `-- router: ${configs.length} llm_configs (seed-default skipped if the superuser has a default), routing on ${routing.length} agents, ${usage.length} token_usage rows`,
  )
  return out
}

/**
 * Contract runs post the last TRACE_WINDOW_DAYS of seed traces to a private Tempo (scripts/seed-live.ts --otlp). The
 * server lists and authorizes a session's traces only through its chat_sessions row (observability/service.rs
 * `get_all_sessions`, `authorize_session`), so those sessions get one, owned by the agent's owner. The detail window
 * is the row's created_at/updated_at ±5 min, so updated_at is the session's last trace end.
 */
function traceChatSessions(seed: ReturnType<typeof generateSeed>): string[] {
  const since = Date.parse(seed.anchor) - TRACE_WINDOW_DAYS * 86_400_000
  const rows = seedTraceSessions(seed, since).map((s) => {
    const last = Math.max(...s.traces.map((t) => t.ts + t.latency_ms))
    return `(${q(s.session_id)}, ${q(s.agent.id)}::uuid, ${q(s.title)}, ${q(new Date(s.created).toISOString())}::timestamptz, ${q(new Date(last).toISOString())}::timestamptz)`
  })
  const out: string[] = []
  for (let i = 0; i < rows.length; i += 400) {
    out.push(`INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title, created_at, updated_at)
SELECT v.sid, a.owner_id, a.id, '/api/agents/' || a.id, v.title, v.created, v.updated
FROM (VALUES
${rows.slice(i, i + 400).join(',\n')}
) AS v(sid, agent_id, title, created, updated)
JOIN agents a ON a.id = v.agent_id
ON CONFLICT (session_id) DO NOTHING;`)
  }
  return out
}

function markerSeed(anchor: string): string {
  return `INSERT INTO llm_configs (id, created_by, name, provider, model, fallback_models, is_default)
SELECT ${q(MARKER_ID)}::uuid, ${SUPERUSER}, ${q(MARKER_PREFIX + anchor)}, 'openai', 'gpt-4o-mini', '[]'::jsonb, false
ON CONFLICT DO NOTHING;`
}

/**
 * MCP rows (plans/feat-mcp.md §8) from the shared seed, so the mock answers recorded fixtures with the same ids. Every
 * dependent row joins its connector, so a skipped server (a name clash) skips them too.
 */
function mcpSeed(seed: ReturnType<typeof generateSeed>): string[] {
  const anchor = Date.parse(seed.anchor)
  const at = (days: number) => q(new Date(anchor - days * 86_400_000).toISOString())
  const live = MCP_CONNECTORS.filter((c) => c.live)
  const idOf = (name: string) => mcpConnectorId(MCP_CONNECTORS.find((c) => c.name === name)!.n)
  const rows = live.map((c) => {
    const status = c.build?.status ?? null
    return `(${q(mcpConnectorId(c.n))}::uuid, ${q(c.provider)}, ${c.owner === 'admin' ? 'true' : 'false'}, ${q(c.name)}, ${q(c.display_name)}, ${q(c.description)}, ${c.provider === 'composio' ? q(`seed-ac-${c.name}`) : 'NULL'}, ${q(c.url)}, ${q(c.auth_type)}, ${q(c.credential_header_name)}, ${q(c.source_kind)}::mcp_connector_source_kind, ${q(status)}, ${status === 'running' ? q(`nasiko/mcp-${c.name}:${c.build!.version}`) : 'NULL'}, ${at(c.daysAgo)}::timestamptz)`
  })
  const tools = live.flatMap((c) =>
    c.tools.map((tl) => `(${q(mcpConnectorId(c.n))}::uuid, ${q(tl.name)}, ${q(tl.description)})`),
  )
  let b = 0
  const builds = live.flatMap((c) => {
    if (!c.build) return []
    const done = (days: number, version: string, st: string, err: string | null) =>
      `(${q(mcpBuildId(++b))}::uuid, ${q(mcpConnectorId(c.n))}::uuid, ${q(version)}, ${st === 'success' ? q(`nasiko/mcp-${c.name}:${version}`) : 'NULL'}, ${q(st)}, ${q(err)}, ${at(days)}::timestamptz)`
    return [
      ...(c.build.prior ? [done(c.daysAgo + 1, c.build.prior, 'success', null)] : []),
      done(
        c.daysAgo,
        c.build.version,
        c.build.status === 'running' ? 'success' : 'failed',
        c.build.error,
      ),
    ]
  })
  const agents = seed.agents.filter((a) => !a.deleted)
  const access = MCP_ACCESS.filter((a) => MCP_CONNECTORS.find((c) => c.name === a.name)?.live).map(
    (a) =>
      `(${q(agents[a.agent]!.id)}::uuid, ${q(idOf(a.name))}::uuid, ${a.enabled}, ${q(JSON.stringify(Object.entries(a.rules).map(([pattern, stance]) => ({ pattern, stance }))))}::jsonb)`,
  )
  const connections = MCP_CONNECTIONS.filter((c) => c.live).map((c) => `(${q(idOf(c.name))}::uuid)`)
  return [
    // Platform rows (owner NULL) clash on name alone, the superuser's on (name, owner).
    `INSERT INTO mcp_connectors (id, provider_type, owner_id, name, display_name, description, auth_config_id, url, auth_type, credential_header_name, source_kind, build_status, container_image_tag, is_active, created_at, updated_at)
SELECT v.id, v.provider, CASE WHEN v.owned THEN ${SUPERUSER} END, v.name, v.display_name, v.description, v.auth_config_id,
       v.url, v.auth_type, COALESCE(v.header, 'Authorization'), v.source_kind, v.build_status, v.image_tag, true, v.created_at, v.created_at
FROM (VALUES
  ${rows.join(',\n  ')}
) AS v(id, provider, owned, name, display_name, description, auth_config_id, url, auth_type, header, source_kind, build_status, image_tag, created_at)
WHERE NOT EXISTS (
  SELECT 1 FROM mcp_connectors m WHERE m.name = v.name
    AND ((NOT v.owned AND m.owner_id IS NULL) OR (v.owned AND m.owner_id = ${SUPERUSER}))
);`,
    `INSERT INTO mcp_connector_tools (connector_id, tool_name, description, last_synced_at)
SELECT v.cid, v.name, v.description, now()
FROM (VALUES
  ${tools.join(',\n  ')}
) AS v(cid, name, description)
JOIN mcp_connectors c ON c.id = v.cid;`,
    `INSERT INTO mcp_connector_builds (id, connector_id, owner_id, version_tag, image_tag, status, error_msg, created_at, completed_at)
SELECT v.id, v.cid, ${SUPERUSER}, v.version, v.image_tag, v.status, v.error, v.at, v.at + interval '90 seconds'
FROM (VALUES
  ${builds.join(',\n  ')}
) AS v(id, cid, version, image_tag, status, error, at)
JOIN mcp_connectors c ON c.id = v.cid;`,
    `INSERT INTO mcp_user_connections (user_id, connector_id, status)
SELECT ${SUPERUSER}, v.cid, 'ACTIVE'
FROM (VALUES ${connections.join(', ')}) AS v(cid)
JOIN mcp_connectors c ON c.id = v.cid;`,
    `INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled, tool_rules)
SELECT v.aid, v.cid, v.enabled, v.rules
FROM (VALUES
  ${access.join(',\n  ')}
) AS v(aid, cid, enabled, rules)
JOIN mcp_connectors c ON c.id = v.cid
JOIN agents a ON a.id = v.aid;`,
    `-- mcp: ${live.length} connectors, ${tools.length} tools, ${builds.length} builds, ${connections.length} connections, ${access.length} agent access rows`,
  ]
}
