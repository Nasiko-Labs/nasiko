// Lab: the real d3 curve type, not the kit's `any`.
import { type CurveFactory, line as d3Line } from 'd3-shape'

export interface SeriesPathPoint {
  x: number
  y: number
  key: string
}

export function computeSeriesPathPoints(
  data: Record<string, unknown>[],
  xAccessor: (datum: Record<string, unknown>) => Date,
  xScale: (value: Date) => number | undefined,
  yScale: (value: number) => number | undefined,
  dataKey: string,
): SeriesPathPoint[] {
  return data.map((datum, index) => {
    const xValue = xAccessor(datum)
    const yValue = datum[dataKey]
    return {
      x: xScale(xValue) ?? 0,
      y: typeof yValue === 'number' ? (yScale(yValue) ?? Number.NaN) : Number.NaN,
      key: String(xValue.getTime?.() ?? index),
    }
  })
}

export function interpolateSeriesPathPoints(
  from: SeriesPathPoint[],
  to: SeriesPathPoint[],
  progress: number,
): SeriesPathPoint[] {
  if (progress >= 1) {
    return to
  }
  if (progress <= 0) {
    return from.length > 0 ? from : to
  }

  const fromByKey = new Map(from.map((point) => [point.key, point]))

  return to.map((target, index) => {
    const source = fromByKey.get(target.key)
    if (source) {
      return {
        key: target.key,
        x: source.x + (target.x - source.x) * progress,
        y: source.y + (target.y - source.y) * progress,
      }
    }

    const previousTarget = index > 0 ? to[index - 1] : undefined
    const previousSource = previousTarget ? fromByKey.get(previousTarget.key) : undefined
    const nextTarget = index < to.length - 1 ? to[index + 1] : undefined
    const nextSource = nextTarget ? fromByKey.get(nextTarget.key) : undefined
    const anchor = previousSource ?? nextSource ?? from[0] ?? target

    return {
      key: target.key,
      x: anchor.x + (target.x - anchor.x) * progress,
      y: anchor.y + (target.y - anchor.y) * progress,
    }
  })
}

export function seriesPathFromPoints(points: SeriesPathPoint[], curve: CurveFactory): string {
  if (points.length === 0) {
    return ''
  }

  const generator = d3Line<SeriesPathPoint>()
    .x((point) => point.x)
    .y((point) => point.y)
    // Nasiko: null values (NaN y) break the line.
    .defined((point) => Number.isFinite(point.y))
    .curve(curve)

  return generator(points) ?? ''
}

export function seriesPathTransitionSignature({
  renderData,
  xAccessor,
  dataKey,
  innerWidth,
  xDomainMin,
  xDomainMax,
}: {
  renderData: Record<string, unknown>[]
  xAccessor: (datum: Record<string, unknown>) => Date
  dataKey: string
  innerWidth: number
  xDomainMin: number
  xDomainMax: number
}): string {
  const values = renderData.map((datum) => {
    const xValue = xAccessor(datum)
    const yValue = datum[dataKey]
    return `${xValue.getTime()}:${typeof yValue === 'number' ? yValue : ''}`
  })

  return `${innerWidth}|${xDomainMin}|${xDomainMax}|${values.join(',')}`
}
