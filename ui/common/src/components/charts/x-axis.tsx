/* eslint-disable react-hooks/refs, react-hooks/set-state-in-effect -- Kit animation code (from the React migration's visx kit): it mounts portals after the
   first render and drives motion springs from values tracked across renders. Correct without the React Compiler,
   so the file opts out of it with 'use no memo'. */
'use client'
'use no memo'

import { memo, useEffect, useMemo, useState } from 'react'
import { createPortal } from 'react-dom'
import { cn } from '@/lib/utils'
import { useChart, useChartStable } from './chart-context'
import { shortDateFmt } from './chart-formatters'
import { DEFAULT_Y_DOMAIN_TWEEN_MS } from './chart-phase'
import { LINE_LOADING_PULSE_EASE } from './line-loading-timing'

const X_AXIS_POSITION_TWEEN_MS = DEFAULT_Y_DOMAIN_TWEEN_MS

export interface XAxisProps {
  /** Number of ticks to show (including first and last). Default: 5. */
  numTicks?: number
  /** Width of the date ticker box for fade calculation. Default: 50 */
  tickerHalfWidth?: number
  /**
   * `"data"` — tick labels snap to data rows so crosshair and tooltip stay aligned (default).
   * `"domain"` — evenly spaced ticks across the time domain (may not align with hover).
   */
  tickMode?: 'domain' | 'data'
}

interface AxisTick {
  date: Date
  x: number
  label: string
}

interface XAxisLabelProps {
  label: string
  x: number
  crosshairX: number | null
  hoveredLabel: string | null
  isHovering: boolean
  tickerHalfWidth: number
  animatePosition: boolean
}

function XAxisLabel({
  label,
  x,
  crosshairX,
  hoveredLabel,
  isHovering,
  tickerHalfWidth,
  animatePosition,
}: XAxisLabelProps) {
  const fadeBuffer = 20
  const fadeRadius = tickerHalfWidth + fadeBuffer

  let opacity = 1
  if (isHovering && crosshairX !== null) {
    const distance = Math.abs(x - crosshairX)
    if (distance < tickerHalfWidth) {
      opacity = 0
    } else if (hoveredLabel && label === hoveredLabel) {
      opacity = 0
    } else if (distance < fadeRadius) {
      opacity = (distance - tickerHalfWidth) / fadeBuffer
    }
  }

  return (
    <div
      className="absolute"
      style={{
        left: x,
        bottom: 12,
        width: 0,
        display: 'flex',
        justifyContent: 'center',
        transition: animatePosition
          ? `left ${X_AXIS_POSITION_TWEEN_MS}ms cubic-bezier(${LINE_LOADING_PULSE_EASE.join(', ')})`
          : undefined,
      }}
    >
      <span
        className={cn('text-chart-label text-xs whitespace-nowrap')}
        style={{
          opacity,
          transition: 'opacity 0.4s ease-in-out',
        }}
      >
        {label}
      </span>
    </div>
  )
}

const MAX_GAP_LAYOUTS = 400

function binomial(n: number, k: number): number {
  if (k < 0 || k > n) {
    return 0
  }
  let result = 1
  for (let i = 0; i < k; i++) {
    result = (result * (n - i)) / (i + 1)
  }
  return result
}

/** All ways to split `span` into `parts` positive integer gaps. */
function composePositiveSum(sum: number, parts: number): number[][] {
  if (parts === 1) {
    return sum >= 1 ? [[sum]] : []
  }

  const layouts: number[][] = []
  for (let gap = 1; gap <= sum - (parts - 1); gap++) {
    for (const tail of composePositiveSum(sum - gap, parts - 1)) {
      layouts.push([gap, ...tail])
    }
  }
  return layouts
}

function gapsToIndices(gaps: number[]): number[] {
  const indices = [0]
  let position = 0
  for (const gap of gaps) {
    position += gap
    indices.push(position)
  }
  return indices
}

function indicesForTickCount(length: number, tickCount: number): number[] {
  const span = length - 1
  if (span <= 0) {
    return [0]
  }

  const rawIndices = Array.from({ length: tickCount }, (_, index) =>
    Math.round((index / (tickCount - 1)) * span),
  )

  const indices = [...new Set(rawIndices)].sort((a, b) => a - b)
  if (indices[0] !== 0) {
    indices.unshift(0)
  }
  if (indices.at(-1) !== span) {
    indices.push(span)
  }

  return [...new Set(indices)].sort((a, b) => a - b)
}

function allIndexLayouts(length: number, tickCount: number): number[][] {
  const span = length - 1
  if (span <= 0) {
    return [[0]]
  }

  const gapCount = tickCount - 1
  if (gapCount <= 0) {
    return [[0]]
  }

  const layoutCount = binomial(span - 1, gapCount - 1)
  if (layoutCount > MAX_GAP_LAYOUTS) {
    return [indicesForTickCount(length, tickCount)]
  }

  return composePositiveSum(span, gapCount).map(gapsToIndices)
}

function dedupeIndicesByLabel(
  indices: number[],
  data: Record<string, unknown>[],
  dateLabels: string[],
  xAccessor: (d: Record<string, unknown>) => Date,
): number[] {
  const seenLabels = new Set<string>()
  const deduped: number[] = []

  for (const index of indices) {
    const point = data[index]
    if (!point) {
      continue
    }
    const label = dateLabels[index] ?? shortDateFmt.format(xAccessor(point))
    if (seenLabels.has(label)) {
      continue
    }
    seenLabels.add(label)
    deduped.push(index)
  }

  return deduped
}

interface TickLayoutScore {
  score: number
  symmetryPenalty: number
  countDistance: number
  /** 0 = smallest gap at end, 1 = at start, 2 = in the middle */
  edgePreference: number
}

function indexGaps(indices: number[]): number[] {
  const gaps: number[] = []
  for (let i = 1; i < indices.length; i++) {
    const current = indices[i]
    const previous = indices[i - 1]
    if (current == null || previous == null) {
      continue
    }
    gaps.push(current - previous)
  }
  return gaps
}

function smallestGapEdgePreference(indices: number[]): number {
  const gaps = indexGaps(indices)
  const smallestGap = Math.min(...gaps)
  const smallestGapIndex = gaps.indexOf(smallestGap)
  if (smallestGapIndex === gaps.length - 1) {
    return 0
  }
  if (smallestGapIndex === 0) {
    return 1
  }
  return 2
}

function scoreTickLayout(
  indices: number[],
  resolveXPx: (index: number) => number,
  targetCount: number,
): TickLayoutScore {
  if (indices.length < 2) {
    return {
      score: Number.POSITIVE_INFINITY,
      symmetryPenalty: Number.POSITIVE_INFINITY,
      countDistance: Number.POSITIVE_INFINITY,
      edgePreference: Number.POSITIVE_INFINITY,
    }
  }

  const pixelGaps: number[] = []
  for (let i = 1; i < indices.length; i++) {
    const current = indices[i]
    const previous = indices[i - 1]
    if (current == null || previous == null) {
      continue
    }
    pixelGaps.push(resolveXPx(current) - resolveXPx(previous))
  }

  const minGap = Math.min(...pixelGaps)
  const maxGap = Math.max(...pixelGaps)
  const meanGap = pixelGaps.reduce((sum, gap) => sum + gap, 0) / pixelGaps.length
  const spreadRatio = meanGap > 0 ? (maxGap - minGap) / meanGap : maxGap - minGap
  const countDistance = Math.abs(indices.length - targetCount)

  const gaps = indexGaps(indices)
  const smallestGap = Math.min(...gaps)
  const smallestGapIndex = gaps.indexOf(smallestGap)
  const interiorPenalty = smallestGapIndex > 0 && smallestGapIndex < gaps.length - 1 ? 0.08 : 0

  const symmetryPenalty =
    gaps.reduce((penalty, gap, index) => {
      return penalty + Math.abs(gap - (gaps.at(-1 - index) ?? gap))
    }, 0) / gaps.length

  return {
    score: spreadRatio + 0.1 * countDistance + interiorPenalty + symmetryPenalty * 0.02,
    symmetryPenalty,
    countDistance,
    edgePreference: smallestGapEdgePreference(indices),
  }
}

function isBetterTickLayout(
  next: TickLayoutScore,
  best: TickLayoutScore,
  nextCountDistance: number,
  bestCountDistance: number,
): boolean {
  if (next.score < best.score - 1e-6) {
    return true
  }
  if (Math.abs(next.score - best.score) > 1e-6) {
    return false
  }
  if (nextCountDistance < bestCountDistance) {
    return true
  }
  if (nextCountDistance > bestCountDistance) {
    return false
  }
  if (next.symmetryPenalty < best.symmetryPenalty - 1e-6) {
    return true
  }
  if (next.symmetryPenalty > best.symmetryPenalty + 1e-6) {
    return false
  }
  return next.edgePreference < best.edgePreference
}

/**
 * Picks tick indices with the most even on-screen spacing. Tries
 * `targetCount ± 1` and evaluates every gap layout when feasible.
 */
function selectEvenlySpacedIndices(
  length: number,
  targetCount: number,
  options?: {
    data?: Record<string, unknown>[]
    dateLabels?: string[]
    xAccessor?: (d: Record<string, unknown>) => Date
    resolveXPx?: (index: number) => number
  },
): number[] {
  if (length <= 0) {
    return []
  }
  if (length === 1) {
    return [0]
  }
  if (length <= targetCount) {
    return Array.from({ length }, (_, index) => index)
  }

  const resolveXPx = options?.resolveXPx ?? ((index: number) => index)

  const minCount = Math.max(2, targetCount - 1)
  const maxCount = Math.min(length, targetCount + 1)

  let bestIndices = indicesForTickCount(length, targetCount)
  let bestScore = scoreTickLayout(bestIndices, resolveXPx, targetCount)
  let bestCountDistance = bestScore.countDistance

  for (let tickCount = minCount; tickCount <= maxCount; tickCount++) {
    for (const rawIndices of allIndexLayouts(length, tickCount)) {
      const indices =
        options?.data && options.dateLabels && options.xAccessor
          ? dedupeIndicesByLabel(rawIndices, options.data, options.dateLabels, options.xAccessor)
          : rawIndices

      if (indices.length < 2) {
        continue
      }

      const layoutScore = scoreTickLayout(indices, resolveXPx, targetCount)
      const countDistance = Math.abs(indices.length - targetCount)

      if (isBetterTickLayout(layoutScore, bestScore, countDistance, bestCountDistance)) {
        bestIndices = indices
        bestScore = layoutScore
        bestCountDistance = countDistance
      }
    }
  }

  return bestIndices
}

function buildDataAlignedTicks({
  data,
  dateLabels,
  marginLeft,
  targetTickCount,
  xAccessor,
  xScale,
}: {
  data: Record<string, unknown>[]
  dateLabels: string[]
  marginLeft: number
  targetTickCount: number
  xAccessor: (d: Record<string, unknown>) => Date
  xScale: (date: Date) => number | undefined
}): AxisTick[] {
  const seenLabels = new Set<string>()
  const ticks: AxisTick[] = []

  const resolveXPx = (index: number) => {
    const point = data[index]
    if (!point) {
      return index
    }
    return xScale(xAccessor(point)) ?? 0
  }

  for (const index of selectEvenlySpacedIndices(data.length, targetTickCount, {
    data,
    dateLabels,
    resolveXPx,
    xAccessor,
  })) {
    const point = data[index]
    if (!point) {
      continue
    }
    const date = xAccessor(point)
    const label = dateLabels[index] ?? shortDateFmt.format(date)
    if (seenLabels.has(label)) {
      continue
    }
    seenLabels.add(label)
    ticks.push({
      date,
      label,
      x: (xScale(date) ?? 0) + marginLeft,
    })
  }

  return ticks
}

function buildDomainTicks({
  marginLeft,
  numTicks,
  xScale,
}: {
  marginLeft: number
  numTicks: number
  xScale: {
    domain: () => Date[]
    (date: Date): number | undefined
  }
}): AxisTick[] {
  const domain = xScale.domain()
  const startDate = domain[0]
  const endDate = domain[1]

  if (!(startDate && endDate)) {
    return []
  }

  const startTime = startDate.getTime()
  const endTime = endDate.getTime()
  const timeRange = endTime - startTime
  const tickCount = Math.max(2, numTicks)
  const seenLabels = new Set<string>()
  const ticks: AxisTick[] = []

  for (let i = 0; i < tickCount; i++) {
    const t = i / (tickCount - 1)
    const date = new Date(startTime + t * timeRange)
    const label = shortDateFmt.format(date)
    if (seenLabels.has(label)) {
      continue
    }
    seenLabels.add(label)
    ticks.push({
      date,
      label,
      x: (xScale(date) ?? 0) + marginLeft,
    })
  }

  return ticks
}

function domainExtendsPastData(
  data: Record<string, unknown>[],
  xAccessor: (d: Record<string, unknown>) => Date,
  xScale: { domain: () => Date[] },
): boolean {
  if (data.length === 0) {
    return false
  }
  const domainEnd = xScale.domain()[1]
  const lastPoint = data.at(-1)
  if (!(domainEnd && lastPoint)) {
    return false
  }
  return domainEnd.getTime() > xAccessor(lastPoint).getTime()
}

/** Domain ticks for the projection tail when brush keeps data-aligned labels. */
function appendProjectionTailTicks(
  ticks: AxisTick[],
  data: Record<string, unknown>[],
  xAccessor: (d: Record<string, unknown>) => Date,
  xScale: {
    domain: () => Date[]
    (date: Date): number | undefined
  },
  marginLeft: number,
  maxExtraTicks: number,
): AxisTick[] {
  if (data.length === 0 || maxExtraTicks <= 0) {
    return ticks
  }

  const lastPoint = data.at(-1)
  const domainEnd = xScale.domain()[1]
  if (!(lastPoint && domainEnd)) {
    return ticks
  }

  const lastDate = xAccessor(lastPoint)
  const startTime = lastDate.getTime()
  const endTime = domainEnd.getTime()
  if (endTime <= startTime) {
    return ticks
  }

  const seenLabels = new Set(ticks.map((tick) => tick.label))
  const extras: AxisTick[] = []
  const extraCount = Math.min(maxExtraTicks, 3)

  for (let i = 1; i <= extraCount; i++) {
    const date = new Date(startTime + (i / (extraCount + 1)) * (endTime - startTime))
    const label = shortDateFmt.format(date)
    if (seenLabels.has(label)) {
      continue
    }
    seenLabels.add(label)
    extras.push({
      date,
      label,
      x: (xScale(date) ?? 0) + marginLeft,
    })
  }

  const endLabel = shortDateFmt.format(domainEnd)
  if (!seenLabels.has(endLabel)) {
    extras.push({
      date: domainEnd,
      label: endLabel,
      x: (xScale(domainEnd) ?? 0) + marginLeft,
    })
  }

  if (extras.length === 0) {
    return ticks
  }

  return [...ticks, ...extras].sort((a, b) => a.x - b.x)
}

export function XAxis(props: XAxisProps) {
  const { containerRef } = useChartStable()
  const [mounted, setMounted] = useState(false)

  useEffect(() => {
    setMounted(true)
  }, [])

  const container = containerRef.current
  if (!(mounted && container)) {
    return null
  }

  return <XAxisInner {...props} container={container} />
}

const XAxisInner = memo(function XAxisInner({
  numTicks = 5,
  tickerHalfWidth = 50,
  tickMode = 'data',
  container,
}: XAxisProps & { container: HTMLDivElement }) {
  const { xScale, margin, tooltipData, data, xAccessor, dateLabels, xDomain } = useChart()

  const labelsToShow = useMemo(() => {
    const projectionExtendsScale =
      tickMode === 'data' && domainExtendsPastData(data, xAccessor, xScale)

    if (tickMode === 'domain') {
      return buildDomainTicks({
        marginLeft: margin.left,
        numTicks,
        xScale,
      })
    }

    // No brush: evenly spaced ticks across the full domain (data + projection).
    if (projectionExtendsScale && xDomain == null) {
      return buildDomainTicks({
        marginLeft: margin.left,
        numTicks,
        xScale,
      })
    }

    const dataTicks = buildDataAlignedTicks({
      data,
      dateLabels,
      marginLeft: margin.left,
      targetTickCount: numTicks,
      xAccessor,
      xScale,
    })

    // Brush: keep data-aligned ticks, add labels only in the projection tail.
    if (projectionExtendsScale && xDomain != null) {
      return appendProjectionTailTicks(
        dataTicks,
        data,
        xAccessor,
        xScale,
        margin.left,
        Math.max(1, numTicks - dataTicks.length + 1),
      )
    }

    return dataTicks
  }, [tickMode, xDomain, data, dateLabels, xAccessor, xScale, margin.left, numTicks])

  const isHovering = tooltipData !== null
  const crosshairX = tooltipData ? tooltipData.x + margin.left : null
  const hoveredLabel =
    isHovering && tooltipData
      ? (dateLabels[tooltipData.index] ?? shortDateFmt.format(xAccessor(tooltipData.point)))
      : null

  return createPortal(
    <div className="pointer-events-none absolute inset-0">
      {labelsToShow.map((item) => (
        <XAxisLabel
          animatePosition={xDomain == null}
          crosshairX={crosshairX}
          hoveredLabel={hoveredLabel}
          isHovering={isHovering}
          key={`${item.date.getTime()}-${item.x}`}
          label={item.label}
          tickerHalfWidth={tickerHalfWidth}
          x={item.x}
        />
      ))}
    </div>,
    container,
  )
})

XAxis.displayName = 'XAxis'

export default XAxis
