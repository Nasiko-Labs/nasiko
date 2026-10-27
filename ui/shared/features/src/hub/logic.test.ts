/**
 * The agent hub's pure rules (plans/feat-agent-centric.md PR1a; eng "Markers", "Sparse buckets", "URL interactions";
 * CEO "Narrative period"): markers from builds, versions, the crash field and the event log; buckets, totals and
 * flags; `?slice=` and `?marker=`; and the sentence, which never ties a number to a change.
 */
import { describe, expect, it } from 'vitest'
import type { AgentVersion } from '@nasiko/features/agents/types'
import type { BuildRecord } from '@nasiko/features/deploy/types'
import type { FinopsSpendTimeseries } from '@nasiko/features/tokenops/types'
import { hubNarrative, type HubNarrativeInput } from '@nasiko/features/narrative/agentHub'
import { resolveWindow } from '@nasiko/features/tokenops/window'
import { laneBand } from '@nasiko/ui/charts/lane-band'
import { latestMarker, markerIdState, markersFor, type MarkerInput } from './events'
import {
  bucketIndexOf,
  flagsFor,
  rowOfSlice,
  rowsFor,
  sliceOf,
  totalsOf,
  weeklyRows,
  withinSessions,
} from './timeline'
import type { AgentEvent } from './types'

const NOW = new Date('2026-03-20T15:00:00Z')
const DAY = 86_400_000
const win = resolveWindow({ preset: '30d' }, NOW)
const ago = (days: number, hours = 0) =>
  new Date(NOW.getTime() - days * DAY - hours * 3_600_000).toISOString()

let n = 0
function build(over: Partial<BuildRecord> = {}): BuildRecord {
  n += 1
  return {
    id: `b-${n}`,
    agent_id: 'a1',
    github_url: null,
    commit_hash: null,
    version_tag: `1.${n}.0`,
    image_reference: 'img',
    status: 'success',
    logs_url: null,
    created_at: ago(3, 1),
    updated_at: ago(3),
    ...over,
  }
}
function version(over: Partial<AgentVersion> = {}): AgentVersion {
  return {
    id: `v-${(n += 1)}`,
    agent_id: 'a1',
    build_id: null,
    can_rollback: true,
    created_at: ago(5),
    image_tag: 'img',
    is_active: false,
    status: 'active',
    version: '0.9.0',
    ...over,
  }
}
const input = (over: Partial<MarkerInput> = {}): MarkerInput => ({
  builds: [],
  versions: [],
  crashedAt: null,
  events: null,
  win,
  harness: false,
  ...over,
})
const event = (over: Partial<AgentEvent> = {}): AgentEvent => ({
  id: 'e1',
  kind: 'restart',
  at: ago(2),
  actor: 'admin',
  summary: 'Restarted',
  before: null,
  after: null,
  ...over,
})

describe('markers', () => {
  it('a finished build and the version it produced are one marker, labelled with the version', () => {
    const b = build({ version_tag: '1.4.2' })
    const set = markersFor(
      input({ builds: [b], versions: [version({ build_id: b.id, version: 'v1.4.2' })] }),
    )
    expect(set.markers).toHaveLength(1)
    expect(set.markers[0]).toMatchObject({
      id: `build:${b.id}`,
      label: 'Build completed',
      version: '1.4.2',
      approximate: true,
    })
  })

  it('a failed build is drawn at its failure time; queued and building records are not drawn', () => {
    const failed = build({ status: 'failed', updated_at: ago(4) })
    const set = markersFor(
      input({ builds: [failed, build({ status: 'queued' }), build({ status: 'building' })] }),
    )
    expect(set.markers.map((m) => m.label)).toEqual(['Build failed'])
    expect(set.markers[0]?.at).toBe(Date.parse(ago(4)))
    expect(set.markers[0]?.approximate).toBe(false)
  })

  it('nothing is called deployed', () => {
    const set = markersFor(input({ builds: [build()], versions: [version()] }))
    expect(set.markers.map((m) => m.label).join(' ')).not.toMatch(/deploy/i)
  })

  it('a version without a build (registry import) is its own marker at its own time', () => {
    const set = markersFor(input({ versions: [version({ version: '2.0.0', created_at: ago(6) })] }))
    expect(set.markers[0]).toMatchObject({
      id: 'version:2.0.0',
      label: 'Version recorded',
      version: '2.0.0',
    })
  })

  it('a version whose build is outside the read is not drawn as build-less', () => {
    const set = markersFor(input({ builds: [build()], versions: [version({ build_id: 'gone' })] }))
    expect(set.markers.map((m) => m.kind)).toEqual(['build'])
  })

  it('without builds access every version is drawn as a version record', () => {
    const set = markersFor(
      input({ builds: null, versions: [version({ build_id: 'x', version: '3.1.0' })] }),
    )
    expect(set.markers.map((m) => m.id)).toEqual(['version:3.1.0'])
  })

  it('20 records whose oldest is inside the window are capped; builds outside the window are counted, not drawn', () => {
    const many = Array.from({ length: 20 }, (_, i) =>
      build({ created_at: ago(i + 1, 1), updated_at: ago(i + 1) }),
    )
    expect(markersFor(input({ builds: many })).buildsCapped).toBe(true)
    const old = [...many.slice(0, 19), build({ created_at: ago(40), updated_at: ago(40) })]
    const set = markersFor(input({ builds: old }))
    expect(set.buildsCapped).toBe(false)
    // Counted, never drawn.
    expect(set.markers).toHaveLength(19)
  })

  it('OSS has no crash marker; EE adds the current crashed_at', () => {
    expect(markersFor(input()).markers).toEqual([])
    const at = ago(1)
    const set = markersFor(input({ crashedAt: at }))
    expect(set.markers[0]).toMatchObject({ id: `crash:${at}`, label: 'Crashed', alert: true })
  })

  it('a harness draws event-log markers only', () => {
    const set = markersFor(
      input({
        harness: true,
        builds: [build()],
        crashedAt: ago(1),
        events: [event({ kind: 'mcp_rule' })],
      }),
    )
    expect(set.markers.map((m) => m.kind)).toEqual(['event'])
  })

  it('event markers carry their kind, actor and alert tone (budget stop)', () => {
    const set = markersFor(
      input({ events: [event({ id: 's', kind: 'budget_stop', actor: null })] }),
    )
    expect(set.markers[0]).toMatchObject({
      id: 'event:s',
      sub: 'budget_stop',
      alert: true,
      actor: null,
    })
  })

  it('one marker per version string, the newest record (a re-published version)', () => {
    const set = markersFor(
      input({
        versions: [
          version({ version: '2.0.0', created_at: ago(9) }),
          version({ version: '2.0.0', created_at: ago(2) }),
        ],
      }),
    )
    expect(set.markers.map((m) => [m.id, m.at])).toEqual([['version:2.0.0', Date.parse(ago(2))]])
  })

  it('an unparseable build time never hides the cap line', () => {
    const many = Array.from({ length: 20 }, (_, i) => build({ updated_at: ago(i + 1) }))
    many[19] = { ...many[19]!, updated_at: 'garbage' }
    expect(markersFor(input({ builds: many })).buildsCapped).toBe(true)
  })

  it('latestMarker is the newest', () => {
    const set = markersFor(
      input({ events: [event({ id: 'old', at: ago(9) }), event({ id: 'new', at: ago(1) })] }),
    )
    expect(latestMarker(set)?.id).toBe('event:new')
  })
})

describe('?marker= (X3)', () => {
  const set = markersFor(input({ events: [event()] }))
  it('keeps an id while the read is pending or failed, finds a drawn one, drops the rest', () => {
    expect(markerIdState(undefined, set)).toBe('none')
    expect(markerIdState('event:e1', null)).toBe('pending')
    expect(markerIdState('event:e1', set)).toBe('found')
    expect(markerIdState('event:nope', set)).toBe('drop')
    expect(markerIdState('junk', null)).toBe('drop')
  })

  it('an id outside the window is dropped (the window never widens)', () => {
    const outside = markersFor(input({ events: [event({ id: 'far', at: ago(45) })] }))
    expect(markerIdState('event:far', outside)).toBe('drop')
  })
})

describe('buckets', () => {
  const series: FinopsSpendTimeseries = {
    bucket: 'day',
    points: [
      {
        bucket_start: `${ago(2).slice(0, 10)}T00:00:00Z`,
        spend_usd: 4,
        operations: 10,
        tool_calls: 0,
        p95_latency_ms: 900,
        top_agent_name: null,
        top_agent_spend_usd: null,
        p50_latency_ms: null,
        p99_latency_ms: null,
      },
      {
        bucket_start: `${ago(1).slice(0, 10)}T00:00:00Z`,
        spend_usd: 6,
        operations: 5,
        tool_calls: 0,
        p95_latency_ms: null,
        top_agent_name: null,
        top_agent_spend_usd: null,
        p50_latency_ms: null,
        p99_latency_ms: null,
      },
    ],
  }

  it('fills omitted buckets with zero spend and requests, keeps a missing p95 a gap', () => {
    const rows = rowsFor({ series, win, createdAt: ago(60) })
    expect(rows.length).toBeGreaterThanOrEqual(30)
    const quiet = rows[0]
    expect(quiet).toMatchObject({ spend: 0, requests: 0, p95: null })
    const t = totalsOf(rows)
    expect(t).toEqual({ peakP95: 900 })
    expect(rows.reduce((a, r) => a + r.spend, 0)).toBe(10)
    expect(rows.every((r) => r.saved === null && r.prevSpend === null)).toBe(true)
  })

  it('sums savings per bucket, never below zero, and fills buckets without a point with 0', () => {
    const at = series.points[0]!.bucket_start
    const rows = rowsFor({
      series,
      win,
      createdAt: ago(60),
      saved: [
        { bucket_start: at, pool_tokens: 10, sent_tokens: 4 },
        { bucket_start: at, pool_tokens: 3, sent_tokens: 9 },
      ],
    })
    expect(rows.find((r) => r.iso === new Date(at).toISOString())?.saved).toBe(6)
    expect(rows.filter((r) => r.saved === 0).length).toBe(rows.length - 1)
  })

  it('peak p95 ignores buckets before the agent existed', () => {
    const early = {
      ...series.points[0]!,
      bucket_start: `${ago(20).slice(0, 10)}T00:00:00Z`,
      p95_latency_ms: 5000,
    }
    const rows = rowsFor({
      series: { ...series, points: [early, ...series.points] },
      win,
      createdAt: ago(10),
    })
    expect(totalsOf(rows).peakP95).toBe(900)
  })

  it('hatches buckets that ended before the agent existed', () => {
    const rows = rowsFor({ series, win, createdAt: ago(10, -2) })
    const created = Date.parse(ago(10, -2))
    expect(rows.filter((r) => r.before).every((r) => r.end <= created)).toBe(true)
    expect(rows.find((r) => r.start <= created && created < r.end)?.before).toBe(false)
  })

  it('draws the previous window by position, daily only', () => {
    const prev = {
      bucket: 'day' as const,
      points: [
        {
          ...series.points[0]!,
          bucket_start: `${win.prevStart.toISOString().slice(0, 10)}T00:00:00Z`,
          spend_usd: 7,
        },
      ],
    }
    const rows = rowsFor({ series, win, createdAt: ago(90), previous: prev })
    expect(rows.find((r) => r.prevSpend === 7)).toBeDefined()
    const day = resolveWindow({ preset: '24h' }, NOW)
    const hourly = rowsFor({
      series: { ...series, bucket: 'hour' },
      win: day,
      createdAt: ago(90),
      previous: prev,
    })
    expect(hourly.every((r) => r.prevSpend === null)).toBe(true)
  })

  it('a time on a bucket edge belongs to the bucket it starts', () => {
    const rows = rowsFor({ series, win, createdAt: ago(90) })
    const r = rows[5]!
    expect(bucketIndexOf(r.start, rows)).toBe(5)
    expect(bucketIndexOf(r.end, rows)).toBe(6)
    expect(bucketIndexOf(rows[0]!.start - 1, rows)).toBe(-1)
  })

  it('merges markers in one bucket into one flag, at the oldest one', () => {
    const rows = rowsFor({ series, win, createdAt: ago(90) })
    const r = rows[10]!
    const set = markersFor(
      input({
        events: [
          event({ id: 'a', at: new Date(r.start + 6 * 3_600_000).toISOString() }),
          event({ id: 'b', at: new Date(r.start + 18 * 3_600_000).toISOString() }),
          event({ id: 'c', at: new Date(rows[11]!.start).toISOString() }),
        ],
      }),
    )
    const flags = flagsFor(set.markers, rows)
    expect(flags.map((f) => [f.index, f.markers.length])).toEqual([
      [10, 2],
      [11, 1],
    ])
    expect(flags[0]?.offset).toBeCloseTo(0.25)
  })

  it('a slice names exactly one bucket of the window, or nothing', () => {
    const rows = rowsFor({ series, win, createdAt: ago(90) })
    expect(rowOfSlice(sliceOf(rows[3]!), rows, win)).toBe(3)
    expect(rowOfSlice('2025-01-01T00:00:00.000Z/2025-01-02T00:00:00.000Z', rows, win)).toBe(-1)
    const r = rows[3]!
    expect(
      rowOfSlice(
        `${new Date(r.start).toISOString()}/${new Date(r.start + 3_600_000).toISOString()}`,
        rows,
        win,
      ),
    ).toBe(-1)
    expect(rowOfSlice('junk', rows, win)).toBe(-1)
  })

  it('phones group days into weeks counted back from the newest day; a week slice names its week', () => {
    const rows = rowsFor({ series, win, createdAt: ago(10, -2) })
    const weeks = weeklyRows(rows)
    const last = weeks.at(-1)!
    expect(last.size).toBe('week')
    expect(last.end - last.start).toBe(7 * DAY)
    expect(last.linkDay).toBe(rows.at(-1)!.start)
    expect(weeks.reduce((a, w) => a + w.spend, 0)).toBe(10)
    expect(weeks.reduce((a, w) => a + w.requests, 0)).toBe(15)
    expect(last.p95).toBe(900)
    expect(weeks[0]?.before).toBe(true)
    expect(rowOfSlice(sliceOf(last), weeks, win)).toBe(weeks.length - 1)
    expect(rowOfSlice(sliceOf(last), rows, win)).toBe(-1)
    // Hourly windows stay hourly.
    const day = resolveWindow({ preset: '24h' }, NOW)
    const hours = rowsFor({ series: { ...series, bucket: 'hour' }, win: day, createdAt: ago(90) })
    expect(weeklyRows(hours)).toHaveLength(hours.length)
  })

  it('Sessions covers the last 7 days of a bucket’s day', () => {
    const rows = rowsFor({ series, win, createdAt: ago(90) })
    expect(withinSessions(rows.at(-1)!, NOW, 7)).toBe(true)
    expect(withinSessions(rows[0]!, NOW, 7)).toBe(false)
  })

  it('one band layout for bars, flags and the axis', () => {
    const b = laneBand(4, 400)
    expect([b.step, b.slot(2), b.center(2), b.indexAt(399), b.indexAt(-5)]).toEqual([
      100, 200, 250, 3, 0,
    ])
    expect(laneBand(0, 400).indexAt(10)).toBe(-1)
  })
})

describe('the sentence', () => {
  const base: HubNarrativeInput = {
    preset: '30d',
    windowLabel: 'Last 30 days',
    win,
    money: { current: 412, previous: 502.44, changePct: -18 },
    createdAt: ago(200),
    latest: null,
  }
  const marker = markersFor(input({ builds: [build({ version_tag: '1.4.2' })] })).markers[0]!

  it('says spend for the window and the change against the one before', () => {
    expect(hubNarrative(base)).toEqual([
      'Spent $412.00 in the last 30 days, 18% less than the 30 before.',
    ])
  })

  it('states the latest change as its own sentence', () => {
    const s = hubNarrative({ ...base, latest: marker })
    expect(s).toHaveLength(2)
    expect(s[1]).toMatch(/^Build v1\.4\.2 completed on /)
  })

  it('drops the comparison when the previous window had no spend or the agent is younger than it', () => {
    expect(hubNarrative({ ...base, money: { current: 5, previous: 0, changePct: null } })[0]).toBe(
      'Spent $5.00 in the last 30 days.',
    )
    expect(hubNarrative({ ...base, createdAt: ago(40) })[0]).toBe(
      'Spent $412.00 in the last 30 days.',
    )
    expect(hubNarrative({ ...base, createdAt: ago(10) })[0]).toMatch(
      /^Spent \$412\.00 since .+, when it was created\.$/,
    )
  })

  it('unpriced calls make the total a floor, with no comparison', () => {
    expect(hubNarrative({ ...base, money: { ...base.money!, unpriced: true } })[0]).toBe(
      'Spent at least $412.00 in the last 30 days.',
    )
    expect(
      hubNarrative({
        ...base,
        money: { current: 0, previous: 0, changePct: null, unpriced: true },
      })[0],
    ).toBe('No priced spend in the last 30 days.')
  })

  // Regression: ISSUE-001 — an event summary ending in "on" read "Token optimization turned on on Sep 30"
  // Found by /qa on 2026-10-11
  it('an event change leads with its date, so a summary ending in "on" reads cleanly', () => {
    const ev = markersFor(
      input({ events: [event({ summary: 'Token optimization turned on', at: ago(3) })] }),
    ).markers[0]!
    const s = hubNarrative({ ...base, latest: ev })[1]
    expect(s).toMatch(/^On [A-Z][a-z]{2} \d{1,2}: Token optimization turned on\.$/)
    expect(s).not.toMatch(/\bon on\b/)
  })

  it('a 24 h change names its day and UTC', () => {
    const s = hubNarrative({ ...base, preset: '24h', windowLabel: 'Last 24 hours', latest: marker })
    expect(s[1]).toMatch(/ on [A-Z][a-z]{2} \d{1,2} \d{2}:\d{2} UTC\.$/)
  })

  it('drops the money sentence when the viewer can’t see it, and the change sentence with no change', () => {
    expect(hubNarrative({ ...base, money: null, latest: marker })).toHaveLength(1)
    expect(hubNarrative({ ...base, money: null })).toEqual([])
  })

  it('never ties a number to a change (Product Principle 1)', () => {
    const kinds = [
      marker,
      ...markersFor(
        input({
          builds: [build({ status: 'failed' })],
          versions: [version()],
          crashedAt: ago(1),
          events: [event()],
        }),
      ).markers,
    ]
    const cases: HubNarrativeInput[] = kinds.flatMap((latest) => [
      { ...base, latest },
      { ...base, latest, preset: '24h' as const, windowLabel: 'Last 24 hours' },
      { ...base, latest, createdAt: ago(3) },
      { ...base, latest, money: { current: 0, previous: 0, changePct: null } },
    ])
    for (const c of cases) {
      const [money, change] = hubNarrative(c)
      // The change sentence uses no connective at all; the money sentence's only "since" introduces the creation date.
      expect(change ?? '').not.toMatch(/\b(since|because|due to|led to|after)\b/i)
      expect(
        (money ?? '').replace(/since [A-Z][a-z]{2} \d{1,2}, when it was created/, ''),
      ).not.toMatch(/\b(since|because|due to|led to|after)\b/i)
    }
  })
})
