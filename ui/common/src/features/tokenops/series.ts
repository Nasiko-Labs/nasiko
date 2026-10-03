import { fmtShortDay, fmtUtcHour } from '@/lib/format'
import type { SpendTimeseriesPoint } from './types'

/**
 * The server returns sparse buckets (only buckets with rows). Charts need real zeros
 * for quiet buckets, otherwise a line bridges an outage (plan A16).
 */
export interface TimelinePoint {
  /** Raw ISO bucket start — clicks resolve by index to this, never to the formatted label (A20). */
  iso: string
  label: string
  spend: number
  operations: number
  p95: number | null
  topAgent: string | null
  topAgentSpend: number | null
}

/** Both bucket sizes are labelled in UTC, matching the UTC day a click opens. */
export function bucketLabel(d: Date, bucket: 'hour' | 'day'): string {
  return bucket === 'hour' ? fmtUtcHour(d) : fmtShortDay(d)
}

/** Bucket cap: a malformed window can never render thousands of bars. */
export const MAX_BUCKETS = 800

function toPoint(
  d: Date,
  bucket: 'hour' | 'day',
  p: SpendTimeseriesPoint | undefined,
): TimelinePoint {
  return {
    iso: d.toISOString(),
    label: bucketLabel(d, bucket),
    spend: p?.spend_usd ?? 0,
    operations: p?.operations ?? 0,
    p95: p?.p95_latency_ms ?? null,
    topAgent: p?.top_agent_name ?? null,
    topAgentSpend: p?.top_agent_spend_usd ?? null,
  }
}

/** Points as returned, without filling — used while a previous window's placeholder data is shown. */
export function toTimeline(
  points: SpendTimeseriesPoint[],
  bucket: 'hour' | 'day',
): TimelinePoint[] {
  return points.map((p) => toPoint(new Date(Date.parse(p.bucket_start)), bucket, p))
}

const HOUR = 3_600_000
const DAY = 24 * HOUR

export function zeroFillTimeline(
  points: SpendTimeseriesPoint[],
  bucket: 'hour' | 'day',
  start: Date,
  end: Date,
): TimelinePoint[] {
  const step = bucket === 'hour' ? HOUR : DAY
  const floor = (t: number) => {
    const d = new Date(t)
    return bucket === 'hour'
      ? Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate(), d.getUTCHours())
      : Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate())
  }
  const byStart = new Map(points.map((p) => [floor(Date.parse(p.bucket_start)), p]))
  const out: TimelinePoint[] = []
  for (
    let t = floor(start.getTime()), n = 0;
    t < end.getTime() && n < MAX_BUCKETS;
    t += step, n++
  ) {
    out.push(toPoint(new Date(t), bucket, byStart.get(t)))
  }
  return out
}
