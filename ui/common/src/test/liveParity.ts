/**
 * Live contract parity (plans/feat-live-contract.md §7.1): every recorded fixture replays through the same MSW
 * handlers the app and page tests use, with the mocks pinned to the fixture's seed anchor and clock, and the mock's
 * response must match the live one in status, content type and shape. Differences the manifest's `allow` lists
 * (with a reason) are accepted; an allowlist entry that no longer matches fails, so the list can't rot.
 *
 * Each edition replays its own fixtures through its own mocks (docs/lab-vs-react-migration-review.md §10.5): the
 * core's test replays `oss/`, the EE layer's replays `ee/` with its mocks stacked in front.
 */
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from 'vitest'
import {
  applyAllowlist,
  compareResponses,
  compareValues,
  describe as describeDiff,
} from '../../../scripts/lib/shape.ts'
import {
  type Endpoint,
  type Fixture,
  loadManifest,
  requestFor,
  tokenContext,
  tokens,
} from '../../../scripts/record-live.ts'
import { configureMocks, resetAgentsMock } from '@/mocks/handlers'
import { generateSeed } from '@/mocks/seed'
import { generateHarnessSeed } from '@/mocks/seed-harness'

const LIVE = join(__dirname, '__live__')
export const manifest = loadManifest()
const byId = new Map(manifest.endpoints.map((e) => [e.id, e]))

export function loadFixtures(edition: 'oss' | 'ee'): Fixture[] {
  const root = join(LIVE, edition)
  const out: Fixture[] = []
  const walk = (d: string) => {
    for (const f of readdirSync(d)) {
      const p = join(d, f)
      if (statSync(p).isDirectory()) walk(p)
      else if (p.endsWith('.json')) out.push(JSON.parse(readFileSync(p, 'utf8')) as Fixture)
    }
  }
  walk(root)
  return out.sort((a, b) => a.id.localeCompare(b.id))
}

export const all = [...loadFixtures('oss'), ...loadFixtures('ee')]
export const fixtures = all.filter((f) => f.source !== 'hand-written')
/** Test names carry the edition: an entry shared by both editions has a fixture per edition. */
export const label = (f: Fixture) => `${f.edition}/${f.id}`
const replayable = fixtures.filter((f) => {
  const e = byId.get(f.id)
  return e && !e.absent && !e.no_replay
})
export const absent = fixtures.filter((f) => byId.get(f.id)?.absent)
export const unknownRoute = all.find((f) => f.id === 'errors.unknown-route' && f.edition === 'oss')
const seeds = new Map<
  string,
  { seed: ReturnType<typeof generateSeed>; harness: ReturnType<typeof generateHarnessSeed> }
>()
export const seedsFor = (anchor: string) => {
  let s = seeds.get(anchor)
  if (!s)
    seeds.set(
      anchor,
      (s = {
        seed: generateSeed({ anchor: new Date(anchor) }),
        harness: generateHarnessSeed({ anchor: new Date(anchor) }),
      }),
    )
  return s
}

/** A plain member of the harness seed's org: what `seed-ee-member` is in the live EE seed. */
const memberPersona = (hs: ReturnType<typeof generateHarnessSeed>) =>
  hs.users.find(
    (u) =>
      u.role === 'member' &&
      !u.is_superuser &&
      u.is_active &&
      !u.service_account &&
      u.unit_ids.length > 0,
  )!.username

/** Replay one fixture's request against the mocks, pinned to its anchor and clock (plan §7.1). */
async function replay(f: Fixture, e: Endpoint) {
  const at = Date.parse(f.recorded_at)
  vi.setSystemTime(at)
  const { seed, harness } = seedsFor(f.anchor)
  // An edition's fixtures replay in its own project, whose mocks answer as its server would; the seed member as a plain member of
  // the harness seed's org.
  configureMocks({
    seed,
    harnessSeed: harness,
    now: () => at,
    loggedIn: f.identity !== 'none',
    variant: null,
    superuser: f.identity !== 'seed-ee-member',
    persona: f.identity === 'seed-ee-member' ? memberPersona(harness) : null,
  })
  const req = requestFor(
    e.replay_path ? { ...e, path: e.replay_path } : e,
    tokens(tokenContext(new Date(f.anchor))),
  )
  const url = new URL(req.path, location.origin)
  for (const [k, v] of Object.entries(req.query)) url.searchParams.set(k, v)
  const res = await fetch(url, { method: e.method, headers: { Accept: 'application/json' } })
  const text = await res.text()
  const content_type = (res.headers.get('content-type') ?? '').split(';')[0]!.trim()
  let body: unknown = text
  if (content_type.includes('json') && text) body = JSON.parse(text)
  return { status: res.status, content_type, body }
}

/** Replay one edition's recorded fixtures through this project's mocks (the edition's own stacked on the core's). */
export function describeReplay(edition: 'oss' | 'ee') {
  beforeAll(() => {
    vi.useFakeTimers({ toFake: ['Date'] })
  })
  afterAll(() => vi.useRealTimers())
  afterEach(() => resetAgentsMock())

  describe(`live parity: ${edition} mock responses match the recorded server`, () => {
    it.each(replayable.filter((f) => f.edition === edition).map((f) => [label(f), f] as const))(
      '%s',
      async (_, f) => {
        const id = f.id
        const e = byId.get(id)!
        const mock = await replay(f, e)
        const diffs = compareResponses(f, mock, e.map_paths)
        // Value rules (plan §7.1): only where both sides derive from the same seed, and only once the shapes agree.
        if (e.values && !diffs.some((d) => d.kind !== 'nullability'))
          diffs.push(...compareValues(f.body, mock.body, e.values === true ? {} : e.values))
        // Nullability depends on data the mock and live seeds don't share everywhere (see scripts/lib/shape.ts); the type
        // contract test decides whether null is allowed. Reported, not failed.
        const nulls = diffs.filter((d) => d.kind === 'nullability')
        if (nulls.length)
          console.info(
            `${id}: nullability differs at ${nulls.map((d) => `${d.path} (live ${d.live}, mock ${d.mock})`).join('; ')}`,
          )
        const { unexplained, stale } = applyAllowlist(
          diffs.filter((d) => d.kind !== 'nullability'),
          e.allow,
        )
        const help = [
          `fixture src/test/__live__/${f.edition}/${e.feature}/${id}.json (recorded ${f.recorded_at})`,
          'next step: fix the mock handler in src/mocks/ for this route,',
          `  or re-record if the server changed: npm run record:live -- --only ${e.feature},`,
          `  or allowlist with a reason in src/test/__live__/manifest.json (${id}.allow)`,
        ].join('\n')
        expect(unexplained.map(describeDiff), help).toEqual([])
        expect(
          stale.map(
            (a) =>
              `allowlisted path ${a.path} no longer differs; remove it from src/test/__live__/manifest.json (${id}.allow)`,
          ),
        ).toEqual([])
      },
    )
  })
}
