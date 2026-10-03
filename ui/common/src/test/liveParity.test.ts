/** Live contract parity (plans/feat-live-contract.md §7.1): fixture coverage for both editions, and the OSS replay. */
import { describe, expect, it } from 'vitest'
import { compareResponses, describe as describeDiff } from '../../../scripts/lib/shape.ts'
import {
  FIXTURE_VERSION,
  recordable,
  REGISTERED_AGENT_INDEX,
  tokenContext,
} from '../../../scripts/record-live.ts'
import { buildAgentsState } from '@/mocks/agents'
import {
  absent,
  describeReplay,
  fixtures,
  label,
  manifest,
  seedsFor,
  unknownRoute,
} from './liveParity'

describe('live parity: fixture coverage', () => {
  it.each(['oss', 'ee'] as const)(
    'has a current fixture for every recordable %s manifest entry, and no orphans',
    (edition) => {
      const want = new Set(recordable(manifest, edition).map((e) => e.id))
      const have = new Set(fixtures.filter((f) => f.edition === edition).map((f) => f.id))
      expect({
        missing: [...want].filter((id) => !have.has(id)),
        orphans: [...have].filter((id) => !want.has(id)),
      }).toEqual({ missing: [], orphans: [] })
      for (const f of fixtures)
        expect(
          f.fixture_version,
          `${f.id}: re-record: fixture format v${f.fixture_version}, expected v${FIXTURE_VERSION}`,
        ).toBe(FIXTURE_VERSION)
    },
  )
})

describe('live parity: request tokens', () => {
  it('{agentRegistered} is an agent the Agents mock never deploys, like every live seed agent', () => {
    const anchor = new Date('2026-03-20T15:00:00Z')
    const { seed, harness } = seedsFor(anchor.toISOString())
    const id = tokenContext(anchor).agentRegistered
    expect(
      buildAgentsState(seed, harness, anchor.getTime()).agents.find((a) => a.id === id),
    ).toMatchObject({ id, status: 'registered', deployment: null })
    expect(seed.agents.indexOf(seed.agents.find((a) => a.id === id)!)).toBe(REGISTERED_AGENT_INDEX)
  })
})

describe('live parity: proposed endpoints answer the unknown-route 404', () => {
  it.each(absent.map((f) => [label(f), f] as const))('%s', (_, f) => {
    expect(unknownRoute, 'errors.unknown-route fixture').toBeDefined()
    expect(f.status).toBe(404)
    expect(compareResponses(f, unknownRoute!).map(describeDiff)).toEqual([])
  })
})

describeReplay('oss')
