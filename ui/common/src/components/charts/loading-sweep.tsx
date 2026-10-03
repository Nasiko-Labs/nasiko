'use client'

// Lab: the real d3 curve type, not the kit's `any`.
import type { CurveFactory } from 'd3-shape'
import { scaleLinear } from '@visx/scale'
import { AreaClosed, LinePath } from '@visx/shape'
import { motion, useReducedMotion } from 'motion/react'
import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import { chartCssVars, useChartStable } from './chart-context'
import { LINE_LOADING_PULSE_EASE, LOADING_LABEL_EXIT_S } from './line-loading-timing'

/**
 * Shared "sweep" loading visuals. A soft diagonal shimmer band travels across a
 * self-contained skeleton silhouette on a loop, painted via an SVG mask. The
 * silhouette re-randomizes between passes (held steady during a pass, re-rolled
 * once the band clears the right edge) so it reads as live and still loading,
 * without warping mid-sweep. Used as the `loadingStyle="sweep"` alternative to
 * the traveling pulse on `<Line>` and `<Area>`, and as the skeleton for
 * `<BarChart status="loading">`.
 */

/** One shimmer sweep, in seconds. */
const DEFAULT_SWEEP_DURATION_S = 2
/** Sweep travel in objectBoundingBox space: off the left edge to off the right. */
const SWEEP_START_X = -1
const SWEEP_END_X = 2
/** Diagonal tilt of the shimmer band, in degrees. */
const SWEEP_ANGLE_DEG = 25
const HEIGHT_MIN_PCT = 20
const HEIGHT_MAX_PCT = 80
const DEFAULT_POINT_COUNT = 14
const BAR_CORNER_RADIUS = 2
const DEFAULT_BAR_COUNT = 12
const DEFAULT_FILL = 'var(--foreground)'
const DEFAULT_BAR_FILL_OPACITY = 0.45
const LINE_STROKE_OPACITY = 0.55
const AREA_FILL_TOP_OPACITY = 0.18
const AREA_FILL_BOTTOM_OPACITY = 0.02
/** Bar width as a fraction of its band (the rest is the inter-bar gap). */
const DEFAULT_BAR_FRACTION = 0.7

// ─── Pure, SSR-safe helpers ──────────────────────────────────────────────
// Heights come from a deterministic hash of (index, seed), never
// `Math.random()`, so the first server render and first client render agree
// (no Next.js hydration mismatch). Re-randomizing only bumps the numeric seed
// on the client, after a sweep completes.

/** Cheap deterministic hash to a fractional part in [0, 1). */
function hashFract(n: number): number {
  const x = Math.sin(n) * 43_758.5453
  return x - Math.floor(x)
}

/** Deterministic heights (percentages of the available height) for a seed. */
function getSkeletonHeights(
  count: number,
  seed = 0,
  min = HEIGHT_MIN_PCT,
  max = HEIGHT_MAX_PCT,
): number[] {
  const range = max - min
  return Array.from(
    { length: count },
    (_, i) => min + Math.floor(hashFract((i + 1) * 12.9898 + seed) * range),
  )
}

/** Deterministic up/down (±1) per bar for the "center" baseline. */
function getSkeletonSigns(count: number, seed = 0): number[] {
  return Array.from({ length: count }, (_, i) =>
    hashFract((i + 1) * 78.233 + seed) < 0.5 ? -1 : 1,
  )
}

/** Bell-curve opacity stops (sin squared) for the shimmer band's soft edges. */
function generateEasedGradientStops(steps = 17, minOpacity = 0.05, maxOpacity = 0.9) {
  return Array.from({ length: steps }, (_, i) => {
    const t = i / (steps - 1)
    const eased = Math.sin(t * Math.PI) ** 2
    const opacity = minOpacity + eased * (maxOpacity - minOpacity)
    return {
      offset: `${(t * 100).toFixed(0)}%`,
      opacity: Number(opacity.toFixed(3)),
    }
  })
}

// ─── Shared mask defs (`${chartId}-mask` is the mask id) ─────────────────────

function LoadingSweepMask({
  chartId,
  width,
  height,
  durationSeconds,
  onSweepComplete,
}: {
  chartId: string
  width: number
  height: number
  durationSeconds: number
  onSweepComplete: () => void
}) {
  const gradientStops = useMemo(() => generateEasedGradientStops(), [])
  const lastXRef = useRef(SWEEP_START_X)

  const handleUpdate = useCallback(
    (latest: { x?: number }) => {
      const xValue = typeof latest.x === 'number' ? latest.x : SWEEP_START_X
      // Re-roll once the band has cleared the visible area (crossed past 1),
      // so the silhouette never changes shape under the user's eye.
      if (xValue >= 1 && lastXRef.current < 1) {
        onSweepComplete()
      }
      lastXRef.current = xValue
    },
    [onSweepComplete],
  )

  return (
    <>
      <linearGradient id={`${chartId}-grad`} x1="0" x2="1" y1="0" y2="0">
        {gradientStops.map(({ offset, opacity }) => (
          <stop key={offset} offset={offset} stopColor="white" stopOpacity={opacity} />
        ))}
      </linearGradient>
      <pattern
        height="1"
        id={`${chartId}-pattern`}
        patternContentUnits="objectBoundingBox"
        patternTransform={`rotate(${SWEEP_ANGLE_DEG})`}
        patternUnits="objectBoundingBox"
        width={3}
        x="0"
        y="0"
      >
        <motion.rect
          animate={{ x: SWEEP_END_X }}
          fill={`url(#${chartId}-grad)`}
          height="1"
          initial={{ x: SWEEP_START_X }}
          onUpdate={handleUpdate}
          transition={{
            duration: durationSeconds,
            ease: 'linear',
            repeat: Number.POSITIVE_INFINITY,
            repeatType: 'loop',
          }}
          width="1"
          y="0"
        />
      </pattern>
      <mask id={`${chartId}-mask`} maskUnits="userSpaceOnUse">
        <rect fill={`url(#${chartId}-pattern)`} height={height} width={width} />
      </mask>
    </>
  )
}

// ─── Line / Area loading sweep (its own re-randomizing silhouette) ───────────

export interface LineLoadingSweepProps {
  /** Curve factory from the host `<Line>` / `<Area>`, so the silhouette matches
   * the chart's interpolation (step, smooth, linear, …). */
  curve: CurveFactory
  /** Fill the silhouette as an area (for `<Area>`); otherwise stroke only. */
  withArea?: boolean
  /** Loading phase: `"loop"` (steady), `"exit"` (loading → ready), or `"enter"`
   * (ready → loading). Exit/enter fade the silhouette and then signal the chart
   * to continue its reveal. Default: `"loop"`. */
  mode?: 'loop' | 'exit' | 'enter'
  /** Fired when an exit/enter transition finishes, to advance the chart phase. */
  onTransitionComplete?: () => void
  stroke?: string
  strokeOpacity?: number
  strokeWidth?: number
  pointCount?: number
  durationSeconds?: number
}

/**
 * Renders a placeholder line/area silhouette (its own, not the chart's skeleton)
 * with the shimmer sweeping across it. The silhouette re-randomizes between
 * passes. Reads inner dimensions from chart context.
 */
export function LineLoadingSweep({
  curve,
  withArea = false,
  mode = 'loop',
  onTransitionComplete,
  stroke = chartCssVars.foreground,
  strokeOpacity = LINE_STROKE_OPACITY,
  strokeWidth = 2,
  pointCount = DEFAULT_POINT_COUNT,
  durationSeconds = DEFAULT_SWEEP_DURATION_S,
}: LineLoadingSweepProps) {
  const { innerWidth, innerHeight } = useChartStable()
  const reduceMotion = useReducedMotion()
  const reactId = useId()
  const chartId = `line-sweep-${reactId.replace(/[^a-zA-Z0-9_-]/g, '')}`
  const isLoop = mode === 'loop'

  const [tick, setTick] = useState(0)
  // Re-randomize only while looping; hold the silhouette steady through a
  // transition so it fades out (or in) as one piece.
  const onSweepComplete = useCallback(() => {
    if (isLoop) {
      setTick((prev) => prev + 1)
    }
  }, [isLoop])
  const heights = useMemo(() => getSkeletonHeights(pointCount, tick), [pointCount, tick])

  // With reduced motion there is no fade to await, so signal the handoff
  // immediately or the phase machine would stall mid-transition.
  useEffect(() => {
    if (reduceMotion && !isLoop) {
      onTransitionComplete?.()
    }
  }, [reduceMotion, isLoop, onTransitionComplete])

  if (innerWidth <= 0 || innerHeight <= 0 || heights.length < 2) {
    return null
  }

  const xScale = scaleLinear({
    domain: [0, heights.length - 1],
    range: [0, innerWidth],
  })
  const yScale = scaleLinear({ domain: [0, 100], range: [innerHeight, 0] })
  const points = heights.map((value, index) => ({ index, value }))
  const getX = (d: { index: number }) => xScale(d.index)
  const getY = (d: { value: number }) => yScale(d.value)

  const silhouette = (
    <>
      {withArea ? (
        <AreaClosed
          curve={curve}
          data={points}
          fill={`url(#${chartId}-area)`}
          x={getX}
          y={getY}
          yScale={yScale}
        />
      ) : null}
      <LinePath
        curve={curve}
        data={points}
        fill="none"
        stroke={stroke}
        strokeLinecap="round"
        strokeOpacity={strokeOpacity}
        strokeWidth={strokeWidth}
        x={getX}
        y={getY}
      />
    </>
  )

  const areaGradient = withArea ? (
    <linearGradient id={`${chartId}-area`} x1="0" x2="0" y1="0" y2="1">
      <stop offset="0%" stopColor={stroke} stopOpacity={AREA_FILL_TOP_OPACITY} />
      <stop offset="100%" stopColor={stroke} stopOpacity={AREA_FILL_BOTTOM_OPACITY} />
    </linearGradient>
  ) : null

  if (reduceMotion) {
    return (
      <>
        {areaGradient ? <defs>{areaGradient}</defs> : null}
        {silhouette}
      </>
    )
  }

  const maskUrl = `url(#${chartId}-mask)`
  const defs = (
    <defs>
      {areaGradient}
      <LoadingSweepMask
        chartId={chartId}
        durationSeconds={durationSeconds}
        height={innerHeight}
        onSweepComplete={onSweepComplete}
        width={innerWidth}
      />
    </defs>
  )

  if (isLoop) {
    return (
      <>
        {defs}
        <g mask={maskUrl}>{silhouette}</g>
      </>
    )
  }

  // Transition: fade the swept silhouette out (exit) or in (enter), then hand
  // off to the chart so it can reveal the real series.
  return (
    <>
      {defs}
      <motion.g
        animate={{ opacity: mode === 'exit' ? 0 : 1 }}
        initial={{ opacity: mode === 'exit' ? 1 : 0 }}
        mask={maskUrl}
        onAnimationComplete={onTransitionComplete}
        transition={{
          duration: LOADING_LABEL_EXIT_S,
          ease: [...LINE_LOADING_PULSE_EASE],
        }}
      >
        {silhouette}
      </motion.g>
    </>
  )
}

LineLoadingSweep.displayName = 'LineLoadingSweep'

// ─── Bar loading skeleton (seeded bars under the sweep, inner coords) ─────────

function SkeletonBars({
  heights,
  signs,
  innerWidth,
  innerHeight,
  baseline,
  barFraction,
  fill,
  fillOpacity,
}: {
  heights: number[]
  signs: number[]
  innerWidth: number
  innerHeight: number
  baseline: 'bottom' | 'center'
  barFraction: number
  fill: string
  fillOpacity: number
}) {
  const bandWidth = innerWidth / heights.length
  const barW = bandWidth * barFraction
  const xOffset = (bandWidth * (1 - barFraction)) / 2
  const isCenter = baseline === 'center'
  const baselineY = isCenter ? innerHeight / 2 : innerHeight
  const halfBarH = isCenter ? innerHeight / 2 : innerHeight

  return (
    <>
      {heights.map((value, i) => {
        const sign = isCenter ? (signs[i] ?? 1) : 1
        const barH = Math.max(1, halfBarH * (value / 100))
        const x = i * bandWidth + xOffset
        const y = sign === 1 ? baselineY - barH : baselineY
        return (
          <rect
            fill={fill}
            fillOpacity={fillOpacity}
            height={barH}
            key={`${x.toFixed(2)}-${value}`}
            rx={BAR_CORNER_RADIUS}
            width={barW}
            x={x}
            y={y}
          />
        )
      })}
    </>
  )
}

export interface BarLoadingSkeletonProps {
  innerWidth: number
  innerHeight: number
  /** Number of skeleton bars. Default: 12 */
  barCount?: number
  /** Bar fill color. Default: `var(--foreground)` */
  fill?: string
  /** Bar fill opacity. Default: 0.45 */
  fillOpacity?: number
  /** Bars rise from the bottom or diverge from the vertical center. Default: `"bottom"` */
  baseline?: 'bottom' | 'center'
  /** Bar width as a fraction of its band (0–1). Default: 0.7 */
  barFraction?: number
  /** One shimmer sweep, in seconds. Default: 2 */
  durationSeconds?: number
}

/**
 * Skeleton bars masked by the shimmer sweep, re-randomizing between passes.
 * Rendered in the chart's inner coordinate space (origin at the inner top-left),
 * so a `BarChart` drops it inside its margin-translated group.
 */
export function BarLoadingSkeleton({
  innerWidth,
  innerHeight,
  barCount = DEFAULT_BAR_COUNT,
  fill = DEFAULT_FILL,
  fillOpacity = DEFAULT_BAR_FILL_OPACITY,
  baseline = 'bottom',
  barFraction = DEFAULT_BAR_FRACTION,
  durationSeconds = DEFAULT_SWEEP_DURATION_S,
}: BarLoadingSkeletonProps) {
  const reduceMotion = useReducedMotion()
  const reactId = useId()
  const chartId = `bar-sweep-${reactId.replace(/[^a-zA-Z0-9_-]/g, '')}`
  const [tick, setTick] = useState(0)
  const onSweepComplete = useCallback(() => setTick((prev) => prev + 1), [])
  const heights = useMemo(() => getSkeletonHeights(barCount, tick), [barCount, tick])
  const signs = useMemo(() => getSkeletonSigns(barCount, tick), [barCount, tick])

  if (innerWidth <= 0 || innerHeight <= 0) {
    return null
  }

  const bars = (
    <SkeletonBars
      barFraction={barFraction}
      baseline={baseline}
      fill={fill}
      fillOpacity={fillOpacity}
      heights={heights}
      innerHeight={innerHeight}
      innerWidth={innerWidth}
      signs={signs}
    />
  )

  if (reduceMotion) {
    return bars
  }

  return (
    <>
      <defs>
        <LoadingSweepMask
          chartId={chartId}
          durationSeconds={durationSeconds}
          height={innerHeight}
          onSweepComplete={onSweepComplete}
          width={innerWidth}
        />
      </defs>
      <g mask={`url(#${chartId}-mask)`}>{bars}</g>
    </>
  )
}

BarLoadingSkeleton.displayName = 'BarLoadingSkeleton'
