// Regression: ISSUE-002 — quiet buckets after reporting began were drawn as "Not recorded"
// Found by /qa on 2026-10-03
// Report: .gstack/qa-reports/qa-report-localhost-2026-10-03.md
import { describe, expect, it } from 'vitest'
import { trendRows } from './lead'

const point = (iso: string, eligible: number, reports: number) => ({
  bucket_start: iso,
  eligible_requests: eligible,
  reports,
  pool_tokens: reports * 100,
  sent_tokens: reports * 40,
})

describe('trendRows: quiet vs not recorded', () => {
  const since = '2026-09-19T00:00:00.000Z'

  it('a bucket with no requests after reporting began is recorded and empty', () => {
    const [quiet] = trendRows([point('2026-10-02T23:00:00.000Z', 0, 0)], since)
    expect(quiet).toMatchObject({ recorded: true, notRecorded: 0, pool: 0, sent: 0, saved: 0 })
  })

  it('a bucket before reporting began is still not recorded', () => {
    const [before] = trendRows([point('2026-09-10T00:00:00.000Z', 0, 0)], since)
    expect(before).toMatchObject({ recorded: false })
    expect(before!.notRecorded).toBeGreaterThan(0)
  })

  it('under half reported is still not recorded, whatever the date', () => {
    const [low] = trendRows([point('2026-10-01T00:00:00.000Z', 10, 3)], since)
    expect(low).toMatchObject({ recorded: false })
  })

  it('without recorded_since a quiet bucket is quiet, not missing', () => {
    expect(trendRows([point('2026-10-01T00:00:00.000Z', 0, 0)])[0]).toMatchObject({
      recorded: true,
    })
  })
})
