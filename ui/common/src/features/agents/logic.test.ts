/**
 * Agents pure logic (plan §10, unit): status matrix, detail normalization, list dedupe,
 * truth-check / roll back timing, the error table, the URL guard, the retired-strings ban
 * and the first-run commands against the recorded `nasiko --help` output.
 */
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { ApiError } from '@/lib/api/client'
import connectHelp from './__fixtures__/nasiko-connect-help.txt?raw'
import deployHelp from './__fixtures__/nasiko-deploy-help.txt?raw'
import newHelp from './__fixtures__/nasiko-new-help.txt?raw'
import { dedupeById, hasFreshDeploying } from './api'
import { copy, errorCopy } from './copy'
import { firstRunCommands } from './format'
import { grantsBodySchema, grantWrite, normalizeGrants } from './grants'
import { isUuid, normalizeDetail, safeHttpsUrl, safeHttpUrl, withFeature } from './normalize'
import { actionsFor, displayStatus, isHarness, STATUS } from './status'
import { GRACE_MS, POLL_MS, WATCH_CAP_MS } from './tuning'
import type { AgentDetailResponse } from './types'
import { expireWatch, isWatching, startWatch, stepWatch, type Watch } from './watch'

describe('status matrix', () => {
  it.each([
    ['running', 'running', ['restart', 'stop']],
    ['deploying', 'deploying', ['stop']],
    ['pending', 'deploying', ['stop']],
    ['crashed', 'attention', ['restart', 'stop']],
    ['failed', 'attention', ['restart', 'stop']],
    ['stopped', 'stopped', ['start']],
    ['registered', 'not-deployed', []],
    ['exploded', 'unknown', []],
    [null, 'unknown', []],
  ] as const)('%s → %s with %j', (raw, display, actions) => {
    expect(displayStatus(raw, false)).toBe(display)
    expect(actionsFor(displayStatus(raw, false))).toEqual(actions)
  })

  it('a harness is always "Coding harness" with no actions, whatever the raw status', () => {
    for (const raw of ['running', 'registered', 'crashed', null]) {
      expect(displayStatus(raw, true)).toBe('harness')
    }
    expect(actionsFor('harness')).toEqual([])
    expect(STATUS.harness.label).toBe('Coding harness')
  })

  it('detects harnesses from the list markers and the detail flag', () => {
    expect(isHarness({ tags: ['coding-agent'] })).toBe(true)
    expect(isHarness({ metadata: { source: 'nasiko-cli-integration' } })).toBe(true)
    expect(isHarness({ is_coding_agent: true })).toBe(true)
    expect(isHarness({ tags: ['search'], metadata: { source: 'upload' } })).toBe(false)
  })
})

describe('normalizeDetail', () => {
  // Recorded shape: camelCase card fields mixed with snake_case server fields.
  const raw = {
    id: 'a1b2c3d4-0000-4000-8000-000000000001',
    name: 'weather',
    display_name: '',
    description: 'Forecasts',
    owner_id: 'u1',
    status: 'running',
    version: '1.2.0',
    url: 'http://weather:8000',
    iconUrl: 'https://example.com/i.png',
    documentation_url: 'https://example.com/docs',
    protocolVersion: '0.3.0',
    preferredTransport: 'JSONRPC',
    defaultInputModes: ['text'],
    default_output_modes: ['text', 42],
    capabilities: { streaming: true, push_notifications: true },
    skills: [
      { id: 's1', name: 'Forecast', examples: ['Weather in Paris?', { bad: 1 }] },
      { nope: true },
      'x',
    ],
    tags: ['weather'],
    can_manage: true,
    is_coding_agent: false,
    created_at: '2026-03-01T00:00:00Z',
    updated_at: '2026-03-02T00:00:00Z',
  } as unknown as AgentDetailResponse

  it('reads both casings into one view model', () => {
    const v = normalizeDetail(raw)
    expect(v.displayName).toBe('weather') // empty display_name falls back to name
    expect(v.iconUrl).toBe('https://example.com/i.png')
    expect(v.documentationUrl).toBe('https://example.com/docs')
    expect(v.inputModes).toEqual(['text'])
    expect(v.outputModes).toEqual(['text'])
    expect(v.capabilities).toEqual({
      streaming: true,
      pushNotifications: true,
      stateTransitionHistory: false,
    })
    expect(v.skills.map((s) => s.name)).toEqual(['Forecast'])
    expect(v.canManage).toBe(true)
    expect(v.isHarness).toBe(false)
  })

  it('tolerates a detail with almost nothing in it', () => {
    const v = normalizeDetail({ id: 'x' } as unknown as AgentDetailResponse)
    expect(v).toMatchObject({
      name: '',
      displayName: '',
      skills: [],
      tags: [],
      canManage: false,
      iconUrl: null,
    })
  })
})

describe('list helpers', () => {
  it('dedupes rows that repeat across pages, keeping the first', () => {
    expect(
      dedupeById([
        { id: 'a', n: 1 },
        { id: 'b', n: 2 },
        { id: 'a', n: 3 },
      ]),
    ).toEqual([
      { id: 'a', n: 1 },
      { id: 'b', n: 2 },
    ])
  })

  it('isUuid accepts only UUIDs', () => {
    expect(isUuid('a1b2c3d4-0000-4000-8000-000000000001')).toBe(true)
    expect(isUuid('weather')).toBe(false)
  })
})

describe('Deploying polling cap', () => {
  const at = Date.parse('2026-03-20T15:00:00Z')
  const row = (status: string, ageMs: number) =>
    ({
      id: 'a',
      name: 'a',
      status,
      tags: [],
      updated_at: new Date(at - ageMs).toISOString(),
    }) as never
  it('polls for a fresh Deploying row, not for one stuck past the cap', () => {
    expect(hasFreshDeploying([row('deploying', POLL_MS)], at)).toBe(true)
    expect(hasFreshDeploying([row('deploying', WATCH_CAP_MS + 1)], at)).toBe(false)
    expect(hasFreshDeploying([row('running', 0)], at)).toBe(false)
  })
})

describe('safeHttpUrl', () => {
  it.each([
    ['https://x.dev/a', 'https://x.dev/a'],
    ['http://x.dev/', 'http://x.dev/'],
    ['javascript:alert(1)', null],
    ['data:text/html,hi', null],
    ['not a url', null],
    ['', null],
    [null, null],
  ])('%s → %s', (input, out) => expect(safeHttpUrl(input)).toBe(out))

  it('icons are https only', () => {
    expect(safeHttpsUrl('https://x.dev/i.png')).toBe('https://x.dev/i.png')
    expect(safeHttpsUrl('http://x.dev/i.png')).toBeNull()
    expect(safeHttpsUrl('javascript:alert(1)')).toBeNull()
  })
})

describe('truth check (restart/start)', () => {
  const t0 = 1_000_000
  const run = (
    kind: 'restart' | 'start',
    polls: [number, Parameters<typeof stepWatch>[1]][],
  ): Watch => polls.reduce((w, [at, s]) => stepWatch(w, s, t0 + at), startWatch(kind, t0))

  it('ignores the synchronous "running" the server writes before the container is up', () => {
    const w = run('restart', [
      [0, 'running'],
      [POLL_MS, 'running'],
    ])
    expect(w.outcome).toBeNull()
    expect(isWatching(w)).toBe(true)
  })

  it('reports done only after Running holds for two polls past the grace period', () => {
    const one = run('restart', [[GRACE_MS, 'running']])
    expect(one.outcome).toBeNull()
    expect(
      run('restart', [
        [GRACE_MS, 'running'],
        [GRACE_MS + POLL_MS, 'running'],
      ]).outcome,
    ).toBe('done')
  })

  it('a non-running poll resets the streak', () => {
    const w = run('start', [
      [GRACE_MS, 'running'],
      [GRACE_MS + POLL_MS, 'deploying'],
      [GRACE_MS + 2 * POLL_MS, 'running'],
    ])
    expect(w.outcome).toBeNull()
    expect(w.stable).toBe(1)
  })

  it('Needs attention after the grace period is a crash', () => {
    expect(run('restart', [[GRACE_MS, 'attention']]).outcome).toBe('crashed')
  })

  it('an early crash reading inside the grace period is ignored (early-poll race)', () => {
    expect(run('restart', [[POLL_MS, 'attention']]).outcome).toBeNull()
  })

  it('times out at the cap without a Running reading', () => {
    expect(run('start', [[WATCH_CAP_MS, 'deploying']]).outcome).toBe('timeout')
  })

  it('a reading past the cap still wins (first poll after a hidden tab)', () => {
    expect(run('restart', [[WATCH_CAP_MS + POLL_MS, 'running']]).outcome).toBe('done')
    expect(run('restart', [[WATCH_CAP_MS + POLL_MS, 'attention']]).outcome).toBe('crashed')
  })

  it('failed polls only end the watch at the cap', () => {
    const w = startWatch('restart', t0)
    expect(expireWatch(w, t0 + GRACE_MS).outcome).toBeNull()
    expect(expireWatch(w, t0 + WATCH_CAP_MS).outcome).toBe('timeout')
    const done = run('restart', [[GRACE_MS, 'attention']])
    expect(expireWatch(done, t0 + WATCH_CAP_MS)).toBe(done)
  })

  it('a finished watch no longer changes', () => {
    const done = run('restart', [[GRACE_MS, 'attention']])
    expect(stepWatch(done, 'running', t0 + WATCH_CAP_MS)).toBe(done)
  })
})

describe('truth check (roll back)', () => {
  const t0 = 5_000_000
  const w0 = startWatch('rollback', t0, 'build-9')

  it('does not finish on the stale Running seen before the task writes Deploying', () => {
    expect(stepWatch(w0, 'running', t0 + POLL_MS).outcome).toBeNull()
  })

  it('finishes once Deploying was seen and the status leaves it', () => {
    const seen = stepWatch(w0, 'deploying', t0 + POLL_MS)
    expect(seen.seenDeploying).toBe(true)
    expect(stepWatch(seen, 'running', t0 + 2 * POLL_MS).outcome).toBe('done')
    expect(stepWatch(seen, 'attention', t0 + 2 * POLL_MS).outcome).toBe('crashed')
  })

  it('a build still queued (never Deploying) is not "Rolled back", even past the grace period', () => {
    expect(stepWatch(w0, 'running', t0 + GRACE_MS).outcome).toBeNull()
    expect(stepWatch(w0, 'running', t0 + WATCH_CAP_MS).outcome).toBe('timeout')
  })

  it('times out while still Deploying at the cap', () => {
    expect(stepWatch(w0, 'deploying', t0 + WATCH_CAP_MS).outcome).toBe('timeout')
  })
})

describe('error table', () => {
  const err = (status: number, body: unknown = null) =>
    new ApiError(status, body, '/api/x', `HTTP ${status}`)

  it.each([
    [401, 'read', /session expired/],
    [403, 'manage', /Only the owner or a superuser/],
    [403, 'lifecycle', /role can't deploy/],
    [404, 'read', /not found/i],
    [400, 'rollback', /can't be rolled back to/],
    [409, 'rollback', /Another build/],
    [422, 'secret', /letters, digits and underscores/],
    [500, 'lifecycle', /couldn't find this agent's container/],
  ] as const)('%i in %s', (status, context, message) => {
    expect(errorCopy(err(status), context).message).toMatch(message)
  })

  it('keeps short plain-text server detail, drops HTML, and truncates long text', () => {
    expect(errorCopy(err(400, 'bad owner'), 'read').detail).toBe('bad owner')
    expect(errorCopy(err(502, '<html>gateway</html>'), 'read').detail).toBeNull()
    expect(errorCopy(err(400, 'x'.repeat(500)), 'read').detail!.length).toBeLessThan(210)
  })

  it('a non-API error is "something went wrong" with its message', () => {
    expect(errorCopy(new Error('boom')).message).toBe(copy.somethingWrong)
  })
})

describe('copy', () => {
  // Retired by the design review (plan §6.6): these misled users on the legacy pages.
  const sources = import.meta.glob(['./**/*.{ts,tsx}', '!./**/*.test.*'], {
    query: '?raw',
    import: 'default',
    eager: true,
  }) as Record<string, string>

  it('scans the agent sources', () => expect(Object.keys(sources).length).toBeGreaterThan(15))

  // 'on this edition': the crash card is field-presence driven, so its wording names no edition (docs/lab-vs-react-migration-review.md §10.5).
  it.each(['Shared with you', 'last deployed', 'Last deployed', 'on this edition'])(
    'no agent component says "%s"',
    (retired) => {
      const hits = Object.entries(sources).filter(
        ([path, src]) => path !== './copy.ts' && src.includes(retired),
      )
      expect(hits.map(([p]) => p)).toEqual([])
      expect(
        Object.values(copy).filter((v) => typeof v === 'string' && v.includes(retired)),
      ).toEqual([])
    },
  )

  it.each(['Registered'])('no agent component renders a "%s" badge', (retired) => {
    const hits = Object.entries(sources).filter(
      ([, src]) => src.includes(`>${retired}<`) || src.includes(`'${retired}'`),
    )
    expect(hits.map(([p]) => p)).toEqual([])
  })

  it('no status label is "Registered"', () => {
    expect(Object.values(STATUS).map((s) => s.label)).not.toContain('Registered')
  })
})

describe('first-run commands match the installed CLI', () => {
  const usage = (help: string) => help.match(/^Usage: (.+)$/m)![1]!
  const cmds = firstRunCommands('https://nasiko.example')

  it('connect takes the page origin as <TARGET>', () => {
    expect(usage(connectHelp)).toBe('nasiko connect [OPTIONS] <TARGET>')
    expect(cmds[0]).toBe('nasiko connect https://nasiko.example')
  })

  it('new takes [TEMPLATE] [NAME], and the template is one the help names', () => {
    expect(usage(newHelp)).toBe('nasiko new [TEMPLATE] [NAME]')
    const [, , template, name] = cmds[1]!.split(' ')
    expect(newHelp).toContain(template!)
    expect(cmds[2]).toContain(name!)
  })

  it('deploy passes the scaffolded directory as the required <IMAGE>', () => {
    expect(usage(deployHelp)).toBe('nasiko deploy [OPTIONS] <IMAGE>')
    expect(deployHelp).toMatch(/<IMAGE>\s+Local Docker image or agent directory/)
    expect(cmds[2]!.split(' ')).toHaveLength(3)
  })
})

describe('grants in both editions (grants.ts, recorded at ea233d20)', () => {
  const recorded = (edition: 'oss' | 'ee') =>
    JSON.parse(
      readFileSync(
        join(__dirname, `../../test/__live__/${edition}/agents/agents.grants.json`),
        'utf8',
      ),
    ) as { body: unknown; request: { path: string } }
  it('reads the OSS summary as is', () => {
    const { body } = recorded('oss')
    expect(normalizeGrants(grantsBodySchema.parse(body), 'a')).toMatchObject({
      edition: 'oss',
      is_public: expect.any(Boolean),
      user_grants: expect.any(Array),
      agent_acl: expect.any(Array),
    })
  })
  it('builds the summary from EE Grant[] rows', () => {
    const { body } = recorded('ee')
    expect(normalizeGrants(grantsBodySchema.parse(body), 'a')).toMatchObject({
      edition: 'ee',
      agent_id: 'a',
      is_public: false,
      user_grants: [],
      agent_acl: [],
      organization: false,
    })
    const rows = [
      {
        id: '1',
        agent_id: 'a',
        grant_type: 'public',
        grantee_id: '*',
        granted_by: null,
        created_at: '',
      },
      {
        id: '2',
        agent_id: 'a',
        grant_type: 'user',
        grantee_id: 'u1',
        granted_by: null,
        created_at: '',
      },
      {
        id: '3',
        agent_id: 'a',
        grant_type: 'agent',
        grantee_id: 't1',
        granted_by: null,
        created_at: '',
      },
      {
        id: '4',
        agent_id: 'a',
        grant_type: 'org_unit',
        grantee_id: 'unit',
        granted_by: null,
        created_at: '',
      },
    ]
    expect(normalizeGrants(rows, 'a')).toEqual({
      edition: 'ee',
      agent_id: 'a',
      is_public: true,
      user_grants: ['u1'],
      agent_acl: ['t1'],
      units: 1,
      organization: false,
    })
  })
  it('rejects anything else', () => {
    expect(grantsBodySchema.safeParse({ available: false }).success).toBe(false)
    expect(grantsBodySchema.safeParse([{ nope: 1 }]).success).toBe(false)
  })
  it('writes with the id in the body on OSS and in the path on EE', () => {
    expect(grantWrite('oss', 'a', 'users', 'u1')).toEqual({
      path: '/api/agents/a/grants/users',
      body: { user_id: 'u1' },
    })
    expect(grantWrite('oss', 'a', 'agents', 't1')).toEqual({
      path: '/api/agents/a/grants/agents',
      body: { agent_id: 't1' },
    })
    expect(grantWrite('ee', 'a', 'users', 'u1')).toEqual({ path: '/api/agents/a/grants/users/u1' })
    // Unknown edition: never guess (an OSS-shaped write to EE reaches the agent through the proxy).
    expect(() => grantWrite(undefined, 'a', 'users', 'u1')).toThrow(/edition/)
  })
})

describe('withFeature', () => {
  it('sets one flag and keeps every other metadata key (the PUT replaces the column)', () => {
    const meta = { source: 'cli', features: { other: 'enabled', prompt_comments: 'enabled' } }
    expect(withFeature(meta, 'prompt_comments', false)).toEqual({
      source: 'cli',
      features: { other: 'enabled', prompt_comments: 'disabled' },
    })
    expect(withFeature({ features: 'junk' }, 'prompt_comments', true)).toEqual({
      features: { prompt_comments: 'enabled' },
    })
  })
})
