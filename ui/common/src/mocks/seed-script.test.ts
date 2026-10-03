/// <reference types="node" />
// @vitest-environment node
/**
 * scripts/seed-trace-usage.ts, run exactly as `npm run seed:sql` does (no DB).
 * The script writes into a developer's real local Postgres, so the safety property is that
 * every DELETE is scoped to seed rows and every inserted row is one a later DELETE matches.
 */
import { spawnSync } from 'node:child_process'
import { describe, expect, it } from 'vitest'

const ANCHOR = '2026-03-20T15:00:00Z'

function run(...args: string[]) {
  const r = spawnSync(process.execPath, ['scripts/seed-trace-usage.ts', ...args], {
    encoding: 'utf8',
    cwd: process.cwd(),
  })
  // Statements are separated by a blank line; none contains one.
  return { status: r.status, stderr: r.stderr, statements: r.stdout.trim().split('\n\n') }
}

const SEED_AGENT = "agent_id::text LIKE '5eed0000-0001-%'"

function expectSeedScopedDeletes(statements: string[]) {
  const deletes = statements.filter((s) => /^DELETE\b/i.test(s))
  expect(deletes).toHaveLength(10)
  for (const d of deletes) expect(d).toMatch(/\bWHERE\b/)
  expect(deletes).toEqual([
    // Harness rows (plan §8): ids 5eed0002-*, removed before the agents they reference.
    "DELETE FROM trace_usage WHERE trace_id LIKE '5eed0002%' AND (agent_id IS NULL OR agent_id::text LIKE '5eed0002-%');",
    // A seed session with real telemetry or a real (unmarked) chat message is kept: deleting it would cascade into them.
    expect.stringMatching(
      /^DELETE FROM chat_sessions WHERE session_id LIKE '5eed0002%' AND NOT \(EXISTS \(SELECT 1 FROM coding_agent_telemetry_events e WHERE e\.session_id = chat_sessions\.session_id\)\n\s+OR EXISTS \(SELECT 1 FROM chat_messages m WHERE m\.session_id = chat_sessions\.session_id AND m\.trace_id IS DISTINCT FROM '5eed0002-message'\)\n\s+OR EXISTS \(SELECT 1 FROM hitl_requests h WHERE h\.chat_session_id = chat_sessions\.session_id\)\);$/,
    ),
    expect.stringMatching(
      /^DELETE FROM agents a WHERE a\.id::text LIKE '5eed0002-%' AND NOT \(EXISTS \(SELECT 1 FROM agent_gateway_tokens[\s\S]*trace_id NOT LIKE '5eed0002%'[\s\S]*session_id NOT LIKE '5eed0002%'[\s\S]*coding_agent_telemetry_events e WHERE e\.agent_id = a\.id\)[\s\S]*mcp_agent_connector_access[\s\S]*agent_grants[\s\S]*agent_acl[\s\S]*a\.llm_config_id IS NOT NULL OR a\.pinned_model IS NOT NULL OR a\.is_public\);$/,
    ),
    // The contract seed's chat sessions (`5eed-sess-*`, --marker), before the seed agents they reference.
    expect.stringMatching(
      /^DELETE FROM chat_sessions WHERE session_id LIKE '5eed-sess-%' AND NOT \(EXISTS \(SELECT 1 FROM coding_agent_telemetry_events/,
    ),
    `DELETE FROM trace_usage WHERE trace_id LIKE '5eed%' AND (${SEED_AGENT} OR (agent_id IS NULL AND agent_name LIKE 'seed-%'));`,
    `DELETE FROM agent_instance_sessions WHERE instance_key LIKE 'seed-%' AND ${SEED_AGENT};`,
    "DELETE FROM agents WHERE id::text LIKE '5eed0000-0001-%';",
    // LLM router: usage, then (after unsetting routing, below) the configs.
    "DELETE FROM token_usage WHERE id::text LIKE '5eed000a-%';",
    "DELETE FROM llm_configs WHERE id::text LIKE '5eed0007-%';",
    // MCP servers (plans/feat-mcp.md §8): tools, grants, connections, agent access and builds cascade with them.
    "DELETE FROM mcp_connectors WHERE id::text LIKE '5eed000e-%';",
  ])
  // Routing that points at a seed config is unset before the configs go (the FK would block the delete).
  const unset =
    "UPDATE agents SET llm_config_id = NULL, pinned_model = NULL WHERE llm_config_id::text LIKE '5eed0007-%';"
  expect(statements.indexOf(unset)).toBeGreaterThan(-1)
  expect(statements.indexOf(unset)).toBeLessThan(
    statements.indexOf("DELETE FROM llm_configs WHERE id::text LIKE '5eed0007-%';"),
  )
  // An adopted seed agent (the CLI reused the row) is reported, not deleted.
  expect(statements.some((s) => /^SELECT 'kept adopted harness agent '/.test(s))).toBe(true)
  // Nothing else may remove or rewrite rows; the only UPDATEs are the router's seed-scoped routing writes.
  expect(statements.join('\n')).not.toMatch(/\b(TRUNCATE|DROP)\b/i)
  for (const u of statements.filter((x) => /\bUPDATE\b/.test(x))) {
    expect(
      u === unset ||
        /^UPDATE agents a SET llm_config_id = c\.id, pinned_model = v\.pin\n[\s\S]*WHERE a\.id = v\.agent_id AND a\.id::text LIKE '5eed0000-0001-%' AND c\.created_by = a\.owner_id AND c\.deleted_at IS NULL;$/.test(
          u,
        ),
      u,
    ).toBe(true)
  }
}

describe('scripts/seed-trace-usage.ts', () => {
  it('--marker adds chat sessions for the traced sessions, all matched by the reset; a plain run adds none', () => {
    const traced = (st: string[]) =>
      st.filter((x) => x.startsWith('INSERT INTO chat_sessions') && x.includes("'5eed-sess-"))
    const small = ['--anchor', ANCHOR, '--days', '8', '--agents', '6', '--spike-day', '0']
    const marked = run(...small, '--marker')
    expect(marked.status).toBe(0)
    const rows = traced(marked.statements).flatMap((x) =>
      [...x.matchAll(/^\('([^']+)'/gm)].map((m) => m[1]!),
    )
    expect(rows.length).toBeGreaterThan(10)
    expect(rows.every((id) => id.startsWith('5eed-sess-'))).toBe(true)
    expect(traced(run(...small).statements)).toEqual([])
  })

  it('--reset emits only BEGIN, the seed-scoped DELETEs and COMMIT', () => {
    const { status, statements } = run('--reset')
    expect(status).toBe(0)
    expect(statements[0]).toBe('BEGIN;')
    expect(statements.at(-1)).toBe('COMMIT;')
    expect(statements).toHaveLength(14)
    expectSeedScopedDeletes(statements)
    expect(statements.join('\n')).not.toMatch(/\bINSERT\b/)
  })

  it('a seed run is one transaction: superuser check, DELETEs, then INSERTs of seed-matchable rows only', () => {
    const { status, statements } = run(
      '--anchor',
      ANCHOR,
      '--days',
      '5',
      '--agents',
      '6',
      '--spike-day',
      '0',
    )
    expect(status).toBe(0)
    expect(statements[0]).toBe('BEGIN;')
    expect(statements.at(-1)).toBe('COMMIT;')
    expect(statements.filter((s) => s === 'BEGIN;' || s === 'COMMIT;')).toHaveLength(2)
    expect(statements[1]).toMatch(/^DO \$\$ BEGIN[\s\S]*RAISE EXCEPTION[\s\S]*no superuser found/)
    expectSeedScopedDeletes(statements)

    const kinds = statements
      .map((s) => (/^DELETE/.test(s) ? 'D' : /^INSERT/.test(s) ? 'I' : '.'))
      .join('')
    expect(kinds.lastIndexOf('D')).toBeLessThan(kinds.indexOf('I'))

    const inserts = statements.filter((s) => s.startsWith('INSERT'))
    const agents = inserts.find((s) =>
      s.startsWith('INSERT INTO agents (id, name, display_name, version'),
    )!
    const agentIds = [...agents.matchAll(/\('([^']*)'::uuid/g)].map((m) => m[1])
    expect(agentIds).toHaveLength(6)
    for (const id of agentIds) expect(id).toMatch(/^5eed0000-0001-/)

    // MCP: every connector is a seed id the reset matches, and every other MCP row hangs off one (ON DELETE CASCADE),
    // agent access only on seed agents.
    const mcp = inserts.filter((x) => /^INSERT INTO mcp_/.test(x))
    expect(mcp.map((x) => x.split(' ')[2])).toEqual([
      'mcp_connectors',
      'mcp_connector_tools',
      'mcp_connector_builds',
      'mcp_user_connections',
      'mcp_agent_connector_access',
    ])
    const connectorIds = [...mcp[0]!.matchAll(/^\s+\('([^']*)'::uuid/gm)].map((m) => m[1])
    expect(connectorIds.length).toBeGreaterThan(0)
    for (const id of connectorIds) expect(id).toMatch(/^5eed000e-/)
    for (const x of mcp.slice(1)) expect(x).toMatch(/JOIN mcp_connectors c ON c\.id = v\.cid/)
    for (const m of mcp[4]!.matchAll(/^\s+\('([^']*)'::uuid/gm))
      expect(m[1]).toMatch(/^5eed0000-0001-/)

    // Every trace row must be matched by the trace_usage DELETE on the next run.
    const traceRows = inserts
      .filter((s) => s.startsWith('INSERT INTO trace_usage') && /\) VALUES\n/.test(s))
      .flatMap((s) => s.split('\n').slice(1))
    expect(traceRows.length).toBeGreaterThan(0)
    for (const row of traceRows) {
      const m =
        /^\('([^']*)', '((?:[^']|'')*)', (?:NULL|'(?:[^']|'')*'), (NULL|'[^']*')::uuid/.exec(row)
      expect(m, row).not.toBeNull()
      const [, traceId, agentName, agentId] = m!
      expect(traceId).toMatch(/^5eed/)
      if (agentId === 'NULL') expect(agentName).toMatch(/^seed-/)
      else expect(agentId).toMatch(/^'5eed0000-0001-/)
    }

    const sessionRows = inserts
      .find((s) => s.startsWith('INSERT INTO agent_instance_sessions'))!
      .split('\n')
      .slice(1)
    for (const row of sessionRows)
      expect(row).toMatch(/^\('5eed0000-0001-[^']*'::uuid, '(?:[^']|'')*', 'seed-/)
  })

  it('router rows: seed-id configs by the superuser, routing only onto owned seed configs, reset-matchable usage', () => {
    const { status, statements } = run(
      '--anchor',
      ANCHOR,
      '--days',
      '5',
      '--agents',
      '6',
      '--spike-day',
      '0',
    )
    expect(status).toBe(0)
    const configs = statements.find((s) => s.startsWith('INSERT INTO llm_configs'))!
    // Created by the same superuser subquery that owns the seed agents, so creator == agent owner.
    expect(configs).toMatch(
      /^INSERT INTO llm_configs [^\n]*\nSELECT v\.id, \(SELECT id FROM users WHERE is_superuser AND deleted_at IS NULL ORDER BY created_at LIMIT 1\), /,
    )
    expect(configs).toMatch(/\nON CONFLICT DO NOTHING;$/)
    const ids = [...configs.matchAll(/^\('([^']*)'::uuid/gm)].map((m) => m[1])
    expect(ids).toHaveLength(4)
    for (const id of ids) expect(id).toMatch(/^5eed0007-/)
    // Platform key everywhere except the deliberate missing secret.
    expect(
      [...configs.matchAll(/'(\[[^']*\])'::jsonb, (NULL|'[A-Z_]+')/g)].map((m) => m[2]),
    ).toEqual(['NULL', 'NULL', 'NULL', "'SEED_MISSING_KEY'"])
    const route = statements.find((s) => s.startsWith('UPDATE agents a SET'))!
    for (const m of route.matchAll(/^\('([^']*)'::uuid, '([^']*)'::uuid/gm)) {
      expect(m[1]).toMatch(/^5eed0000-0001-/)
      expect(m[2]).toMatch(/^5eed0007-/)
    }
    const usage = statements.filter((s) => s.startsWith('INSERT INTO token_usage'))
    expect(usage.length).toBeGreaterThan(0)
    for (const u of usage) {
      expect(u).toMatch(/JOIN agents a ON a\.id = v\.agent_id;$/)
      for (const m of u.matchAll(/^\('([^']*)'::uuid, '([^']*)'::uuid/gm)) {
        expect(m[1]).toMatch(/^5eed000a-/)
        expect(m[2]).toMatch(/^5eed0000-0001-/)
      }
    }
  })

  it('harness rows: guarded coding-agent agents (5eed0002-*), names computed in SQL, activity only for inserted agents', () => {
    const { status, statements } = run(
      '--anchor',
      ANCHOR,
      '--days',
      '5',
      '--agents',
      '6',
      '--spike-day',
      '0',
    )
    expect(status).toBe(0)
    const harnessAgents = statements.filter((s) =>
      s.startsWith(
        'INSERT INTO agents (id, name, display_name, description, owner_id, tags, metadata, coding_agent_integration_id)',
      ),
    )
    expect(harnessAgents.length).toBeGreaterThan(0)
    for (const a of harnessAgents) {
      expect(a).toMatch(
        /SELECT '5eed0002-[^']*'::uuid, btrim\(regexp_replace\(lower\(u\.username\), '\[\^a-z0-9\]\+', '-', 'g'\), '-'\) \|\| '-(claude-code|codex|opencode|cursor)'/,
      )
      // Skipped (not an error) when the superuser already has a live agent with this name or integration.
      expect(a).toMatch(
        /AND NOT EXISTS \(SELECT 1 FROM agents x WHERE x\.owner_id = u\.id AND x\.deleted_at IS NULL/,
      )
      // A kept (adopted, maybe soft-deleted) seed agent already holds the fixed id: skip, never a PK error.
      expect(a).toMatch(
        /AND NOT EXISTS \(SELECT 1 FROM agents y WHERE y\.id = '5eed0002-[^']*'::uuid\);$/,
      )
      expect(a).toMatch(/x\.coding_agent_integration_id = '(claude|codex|opencode|cursor)'/)
    }
    const activity = statements.filter(
      (s) => /^INSERT INTO (trace_usage|chat_sessions) \(/.test(s) && /\nSELECT/.test(s),
    )
    expect(activity.length).toBeGreaterThan(0)
    for (const s of activity) {
      // Joined to agents (a skipped agent inserts nothing) and never onto an adopted real agent.
      // All four adoption signals guard the seed traffic (token, real traces, real sessions, telemetry).
      expect(s).toMatch(
        /JOIN agents a ON a\.id = v\.agent_id\nWHERE NOT \(EXISTS \(SELECT 1 FROM agent_gateway_tokens[\s\S]*trace_id NOT LIKE '5eed0002%'[\s\S]*session_id NOT LIKE '5eed0002%'[\s\S]*coding_agent_telemetry_events[\s\S]*a\.is_public\)(\nON CONFLICT \(session_id\) DO NOTHING)?;$/,
      )
      // Every row's key and agent id are reset-matchable.
      for (const m of s.matchAll(/^\('([^']*)'/gm)) expect(m[1]).toMatch(/^5eed0002/)
      for (const m of s.matchAll(/'([^']*)'::uuid/g)) expect(m[1]).toMatch(/^5eed0002-/)
    }
    // chat_messages: marked as seed rows, only for seed sessions without messages, and batched by whole session.
    const messages = statements.filter((s) => s.startsWith('INSERT INTO chat_messages'))
    expect(messages.length).toBeGreaterThan(0)
    const seen = new Map<string, number>()
    messages.forEach((m, i) => {
      expect(m).toMatch(/'5eed0002-message'\nFROM \(VALUES/)
      expect(m).toMatch(
        /WHERE v\.sid LIKE '5eed0002%' AND NOT EXISTS \(SELECT 1 FROM chat_messages m WHERE m\.session_id = v\.sid\);$/,
      )
      for (const r of m.matchAll(/^\('([^']*)'/gm)) {
        expect(r[1]).toMatch(/^5eed0002/)
        expect(seen.get(r[1]!) ?? i).toBe(i) // a session never spans two batches
        seen.set(r[1]!, i)
      }
    })
    const traces = statements.filter(
      (s) => s.startsWith('INSERT INTO trace_usage') && /\nSELECT/.test(s),
    )
    expect(traces[0]).toMatch(/a\.owner_id, v\.model/) // user_id = owner
    // One row per turn (operations = COUNT(*) on the server): trace ids are <session>-<turn>.
    expect(traces.join('\n')).toMatch(
      /^\('5eed0002-0004-[^']*-\d{2}', '5eed0002-0004-[^']*', '5eed0002-/m,
    )
  })

  it.each([
    [['--days', 'x'], /--days must be an integer >= 1, got "x"/],
    [['--agents', '0'], /--agents must be an integer >= 1, got "0"/],
    [['--spike-day=-1'], /--spike-day must be an integer >= 0/],
    [['--anchor', 'nope'], /--anchor must be an ISO date\/time, got "nope"/],
  ])('bad flags %j exit 2 with a message and no SQL', (args, message) => {
    const r = spawnSync(process.execPath, ['scripts/seed-trace-usage.ts', ...args], {
      encoding: 'utf8',
      cwd: process.cwd(),
    })
    expect(r.status).toBe(2)
    expect(r.stderr).toMatch(message)
    expect(r.stdout).toBe('')
  })
})
