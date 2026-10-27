/**
 * Largest-Triangle-Three-Buckets over one y accessor: the indices to keep,
 * first and last always included.
 */
function lttbIndices(len: number, maxPoints: number, getY: (index: number) => number): number[] {
  const kept = [0]
  const bucketSize = (len - 2) / (maxPoints - 2)
  let previous = 0
  for (let i = 0; i < maxPoints - 2; i++) {
    const rangeStart = Math.floor((i + 1) * bucketSize) + 1
    const rangeEnd = Math.min(Math.floor((i + 2) * bucketSize) + 1, len - 1)
    const nextStart = Math.floor((i + 2) * bucketSize) + 1
    const nextEnd = Math.min(Math.floor((i + 3) * bucketSize) + 1, len)
    const nextCount = Math.max(0, nextEnd - nextStart)

    let avgX = len - 1
    let avgY = getY(len - 1)
    if (nextCount > 0) {
      avgX = 0
      avgY = 0
      for (let j = nextStart; j < nextEnd; j++) {
        avgX += j
        avgY += getY(j)
      }
      avgX /= nextCount
      avgY /= nextCount
    }

    const ay = getY(previous)
    let maxArea = -1
    let maxIndex = rangeStart
    for (let j = rangeStart; j < rangeEnd; j++) {
      const area = Math.abs((previous - avgX) * (getY(j) - ay) - (previous - j) * (avgY - ay)) * 0.5
      if (area > maxArea) {
        maxArea = area
        maxIndex = j
      }
    }
    kept.push(maxIndex)
    previous = maxIndex
  }
  kept.push(len - 1)
  return kept
}

/**
 * Down-sample a time series for rendering. Each series in `valueKeys` is
 * decimated on its own and the kept points are unioned, so a spike in one
 * series survives even when the others are flat (decimating the mean of the
 * series, as the kit did, averaged spikes away). The result can exceed
 * `maxPoints` by up to a factor of `valueKeys.length`.
 */
export function decimateTimeSeries<T extends Record<string, unknown>>(
  data: T[],
  maxPoints: number,
  valueKeys: string[] = [],
): T[] {
  const len = data.length
  if (maxPoints >= len || maxPoints < 3) {
    return data
  }
  const numeric = (v: unknown, index: number) => (typeof v === 'number' ? v : index)
  const accessors: ((index: number) => number)[] =
    valueKeys.length > 0
      ? valueKeys.map((key) => (i) => numeric(data[i]?.[key], i))
      : [
          (i) => {
            const first = Object.values(data[i] ?? {}).find((v) => typeof v === 'number')
            return numeric(first, i)
          },
        ]
  const keep = new Set<number>()
  for (const getY of accessors) {
    for (const i of lttbIndices(len, maxPoints, getY)) keep.add(i)
  }
  return [...keep].sort((a, b) => a - b).map((i) => data[i] as T)
}

/** ~1.5 points per pixel — enough for crisp curves without over-drawing. */
export function maxRenderPointsForWidth(innerWidth: number): number {
  return Math.max(64, Math.ceil(innerWidth * 1.5))
}

/** Bucket OHLC rows into fewer candles while preserving high/low extremes. */
export function decimateOhlcData<T extends Record<string, unknown>>(
  data: T[],
  maxPoints: number,
): T[] {
  const len = data.length
  if (maxPoints >= len || maxPoints < 2) {
    return data
  }

  const bucketSize = len / maxPoints
  const sampled: T[] = []

  for (let i = 0; i < maxPoints; i++) {
    const start = Math.floor(i * bucketSize)
    const end = Math.min(len, Math.floor((i + 1) * bucketSize))
    if (start >= end) {
      continue
    }

    const bucket = data.slice(start, end)
    const first = bucket[0] as T
    const last = bucket.at(-1) as T

    let high = Number.NEGATIVE_INFINITY
    let low = Number.POSITIVE_INFINITY
    for (const row of bucket) {
      const rowHigh = row.high
      const rowLow = row.low
      if (typeof rowHigh === 'number' && rowHigh > high) {
        high = rowHigh
      }
      if (typeof rowLow === 'number' && rowLow < low) {
        low = rowLow
      }
    }

    sampled.push({
      ...last,
      open: first.open,
      high: Number.isFinite(high) ? high : last.high,
      low: Number.isFinite(low) ? low : last.low,
      close: last.close,
    } as T)
  }

  return sampled
}
