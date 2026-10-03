'use client'

import type { scaleBand } from '@visx/scale'
import type { Transition } from 'motion/react'
import { motion, useReducedMotion } from 'motion/react'
import { memo, useId, useMemo } from 'react'
import { barDepthAndRise, barDepthMaxDepth } from './bar-depth-geometry'
import { BarEdge } from './bar-edge'
import { chartCssVars, useChart, useChartStable, useYScale } from './chart-context'
import { useChartLegendHover } from './chart-legend-hover'
import { transitionWithDelay } from './motion-utils'

type ScaleBand<Domain extends { toString(): string }> = ReturnType<typeof scaleBand<Domain>>

export type BarLineCap = 'round' | 'butt' | number
export type BarAnimationType = 'grow' | 'fade'

// ── Bar-depth perspective trim ───────────────────────────────────────────
// Uses the SHARED geometry (`bar-depth-geometry.ts`) so a
// `<Bar perspective>` front face lines up exactly with
// `<BarDepthBack>`'s lid — the formula lives in one place for both.

/** perspectiveRise for a positive bar whose visual top sits at `topY`.
 * Returns 0 for a dead-center bar or a dense chart (degenerate depth). */
function barDepthPerspectiveRise(
  barScale: ScaleBand<string>,
  bandWidth: number,
  barXAccessor: (d: Record<string, unknown>) => string,
  innerWidth: number,
  datum: Record<string, unknown>,
  topY: number,
  baselineY: number,
): number {
  const centerX = innerWidth / 2
  if (centerX <= 0) {
    return 0
  }
  const step = (barScale as unknown as { step?: () => number }).step?.() ?? bandWidth
  const maxDepth = barDepthMaxDepth(step, bandWidth)
  const bandX = barScale(barXAccessor(datum)) ?? 0
  const cx = bandX + bandWidth / 2
  const absOffset = Math.min(1, Math.abs((cx - centerX) / centerX))
  const naturalHeight = Math.abs(baselineY - topY)
  return barDepthAndRise(absOffset, naturalHeight, maxDepth).perspectiveRise
}

export interface BarProps {
  /** Key in data to use for y values */
  dataKey: string
  /** Y-scale group id for vertical bars (Recharts `yAxisId`). Default: `"left"`. */
  yAxisId?: string | number
  /** Fill color for the bar. Can be a color, gradient url, or pattern url. Default: var(--chart-line-primary) */
  fill?: string
  /** Lab: a 2 px top edge in this colour (vertical bars; `--chart-n-edge`). */
  edge?: string
  /** Color for tooltip dot. Use when fill is a gradient/pattern. Default: uses fill value */
  stroke?: string
  /** Line cap style for bar ends: "round", "butt", or a number for custom radius. Default: "round" */
  lineCap?: BarLineCap
  /** Whether to animate the bars. Default: true */
  animate?: boolean
  /** Animation type: "grow" (height) or "fade" (opacity + blur). Default: "grow" */
  animationType?: BarAnimationType
  /** Opacity when not hovered (when another bar is hovered). Default: 0.3 */
  fadedOpacity?: number
  /** Stagger delay between bars in seconds. Auto-calculated if not provided. */
  staggerDelay?: number
  /** Gap between stacked bars in pixels. Default: 0 */
  stackGap?: number
  /** Gap between grouped bars in pixels. Default: 4 */
  groupGap?: number
  /** Shrink each positive bar's top by its perspective rise so the front face
   * lines up with `<BarDepthBack>`'s lid (instead of the lid sitting above the
   * front face). Pass `true` whenever the chart also renders the bar-depth 3D
   * surfaces. Default: false */
  perspective?: boolean
  /** Minimum rendered bar height in px (non-stacked, vertical). Floors short or
   * zero-value bars so they stay visible. Pair with the same value on
   * `<BarDepthProvider minBarHeight>` when using the 3D surfaces. Default: 0 */
  minBarHeight?: number
}

interface BarInnerProps extends BarProps {
  barScale: ScaleBand<string>
  bandWidth: number
  barXAccessor: (d: Record<string, unknown>) => string
}

interface AnimatedBarProps {
  x: number
  y: number
  width: number
  height: number
  fill: string
  rx: number
  ry: number
  index: number
  isFaded: boolean
  animationType: BarAnimationType
  innerHeight: number
  fadedOpacity: number
  staggerDelay: number
  enterTransition?: Transition
  revealEpoch: number
  isHorizontal: boolean
  edge?: string
}

function AnimatedBar({
  x,
  y,
  width,
  height,
  fill,
  rx,
  ry,
  index,
  isFaded,
  animationType,
  innerHeight,
  fadedOpacity,
  staggerDelay,
  enterTransition,
  revealEpoch,
  isHorizontal,
  edge,
}: AnimatedBarProps) {
  const enterAnim = transitionWithDelay(enterTransition, index * staggerDelay)

  if (animationType === 'fade') {
    return (
      <motion.rect
        animate={{
          opacity: isFaded ? fadedOpacity : 1,
          filter: 'blur(0px)',
        }}
        fill={fill}
        height={height}
        initial={{ opacity: 0, filter: 'blur(2px)' }}
        key={`fade-${index}-${revealEpoch}`}
        rx={rx}
        ry={ry}
        transition={enterAnim}
        width={width}
        x={x}
        y={y}
      />
    )
  }

  const initial = isHorizontal
    ? { width: 0, height, x: 0, y }
    : { width, height: 0, x, y: innerHeight }
  const target = isHorizontal ? { width, height, x: 0, y } : { width, height, x, y }

  return (
    <g opacity={isFaded ? fadedOpacity : 1} style={{ transition: 'opacity 0.15s ease-in-out' }}>
      <motion.rect
        animate={target}
        fill={fill}
        initial={initial}
        key={`grow-${index}-${revealEpoch}`}
        rx={rx}
        ry={ry}
        transition={enterAnim}
      />
      {isHorizontal ? null : (
        <BarEdge
          edge={edge}
          growFrom={innerHeight}
          height={height}
          key={`edge-${index}-${revealEpoch}`}
          transition={enterAnim}
          width={width}
          x={x}
          y={y}
        />
      )}
    </g>
  )
}

const BarInner = memo(function BarInner({
  dataKey,
  yAxisId,
  fill = chartCssVars.linePrimary,
  edge,
  lineCap = 'round',
  animate = true,
  animationType = 'grow',
  fadedOpacity = 0.3,
  staggerDelay,
  stackGap = 0,
  groupGap = 4,
  perspective = false,
  minBarHeight = 0,
  barScale,
  bandWidth,
  barXAccessor,
}: BarInnerProps) {
  // Lab: reduced motion draws bars at rest (the kit grew them regardless).
  const reducedMotion = useReducedMotion() === true
  const {
    data,
    yScale: chartYScale,
    innerHeight,
    innerWidth,
    isLoaded,
    hoveredBarIndex,
    lines,
    orientation,
    stacked,
    stackOffsets,
    animationDuration,
    enterTransition,
    revealEpoch = 0,
  } = useChart()

  // Calculate stagger delay automatically if not provided
  // Total animation duration is ~1200ms, with 40% for stagger spread and 60% for bar animation
  const totalAnimDuration = animationDuration || 1100
  const staggerSpread = totalAnimDuration * 0.4 // 40% of time for stagger spread
  const calculatedStaggerDelay =
    staggerDelay ?? (data.length > 1 ? staggerSpread / 1000 / data.length : 0)
  const uniqueId = useId()

  const isHorizontal = orientation === 'horizontal'

  // Find the index of this bar series among all bar series
  const { hoveredIndex: legendHoveredIndex } = useChartLegendHover()

  const seriesIndex = useMemo(() => {
    const idx = lines.findIndex((l) => l.dataKey === dataKey)
    return idx >= 0 ? idx : 0
  }, [lines, dataKey])

  const seriesConfig = lines[seriesIndex]
  const valueScale = useYScale(yAxisId ?? seriesConfig?.yAxisId)

  const isLegendDimmed = legendHoveredIndex !== null && legendHoveredIndex !== seriesIndex

  const seriesCount = lines.length
  const isLastSeries = seriesIndex === seriesCount - 1

  // Calculate the width for each bar within a group (for non-stacked)
  const barWidth = useMemo(() => {
    if (!bandWidth || seriesCount === 0) {
      return 0
    }
    if (stacked) {
      // Stacked bars use full band width
      return bandWidth
    }
    // Leave a gap between grouped bars (controlled by groupGap prop)
    const effectiveGroupGap = seriesCount > 1 ? groupGap : 0
    return (bandWidth - effectiveGroupGap * (seriesCount - 1)) / seriesCount
  }, [bandWidth, seriesCount, stacked, groupGap])

  // Calculate corner radius based on lineCap. Perspective bars force a flat
  // top (radius 0) so the 3D lid from `<BarDepthBack>` meets the bar with no
  // gap — rounded corners would leave a wedge, so `perspective` overrides it.
  const cornerRadius = useMemo(() => {
    if (perspective) {
      return 0
    }
    if (typeof lineCap === 'number') {
      return lineCap
    }
    if (lineCap === 'round' && barWidth) {
      return Math.min(barWidth / 2, 8)
    }
    return 0
  }, [lineCap, barWidth, perspective])

  return (
    <g className={`bar-series-${uniqueId}`}>
      {data.map((d, i) => {
        const value = d[dataKey]
        if (typeof value !== 'number') {
          return null
        }

        const categoryValue = barXAccessor(d)
        const bandPos = barScale(categoryValue) ?? 0

        let x: number
        let y: number
        let barHeight: number
        let barW: number

        const scale = isHorizontal ? chartYScale : valueScale

        if (isHorizontal) {
          // Horizontal bars: category on y-axis, value on x-axis
          const valuePos = scale(value) ?? 0
          barW = valuePos // Width is the value position (grows from left)
          barHeight = barWidth

          if (stacked && stackOffsets) {
            const offset = stackOffsets.get(i)?.get(dataKey) ?? 0
            x = scale(offset) ?? 0
            barW = valuePos - x
            // Apply stack gap for horizontal: shift right and reduce width
            const gapOffset = seriesIndex * stackGap
            x += gapOffset
            if (!isLastSeries && stackGap > 0) {
              barW = Math.max(0, barW - stackGap)
            }
          } else {
            x = 0
            // For grouped bars, offset y position
            const effectiveGroupGap = seriesCount > 1 ? groupGap : 0
            y = bandPos + seriesIndex * (barWidth + effectiveGroupGap)
          }
          y = stacked
            ? bandPos
            : bandPos + seriesIndex * (barWidth + (seriesCount > 1 ? groupGap : 0))
        } else {
          // Vertical bars: category on x-axis, value on y-axis
          const valuePos = scale(value) ?? 0
          barHeight = innerHeight - valuePos
          barW = barWidth

          if (stacked && stackOffsets) {
            const offset = stackOffsets.get(i)?.get(dataKey) ?? 0
            const offsetY = scale(offset) ?? innerHeight
            // Apply stack gap: shift up and reduce height
            const gapOffset = seriesIndex * stackGap
            y = offsetY - barHeight - gapOffset
            // Reduce height slightly for non-last bars to create visual gap
            if (!isLastSeries && stackGap > 0) {
              barHeight = Math.max(0, barHeight - stackGap)
            }
          } else {
            y = valuePos
            // For grouped bars, offset x position
            const effectiveGroupGap = seriesCount > 1 ? groupGap : 0
            x = bandPos + seriesIndex * (barWidth + effectiveGroupGap)
          }
          x = stacked
            ? bandPos
            : bandPos + seriesIndex * (barWidth + (seriesCount > 1 ? groupGap : 0))

          // Minimum visible height — floor short/zero non-stacked bars so a
          // zero-value data point still reads as a tiny bar instead of
          // vanishing. Grows up from the baseline. Floored bars skip the
          // perspective trim (sub-pixel on a 3px bar; keeps the front aligned
          // with bar-depth, which also skips trim for floored bars).
          let isFloored = false
          if (!stacked && minBarHeight > 0 && value >= 0 && barHeight < minBarHeight) {
            const baselineY = scale(0) ?? innerHeight
            barHeight = minBarHeight
            y = baselineY - minBarHeight
            isFloored = true
          }

          // Perspective trim — shrink the topmost positive bar's front-face
          // top down by its perspective rise so it meets `<BarDepthBack>`'s
          // lid back edge. Stacked: only the last (topmost) series; grouped or
          // single: every positive bar. Clamped to `barHeight - 1` so very
          // short bars keep a positive height (matches bar-depth's clamp).
          if (perspective && value > 0 && !isFloored && (!stacked || isLastSeries)) {
            const baselineY = scale(0) ?? innerHeight
            const rise = barDepthPerspectiveRise(
              barScale,
              bandWidth,
              barXAccessor,
              innerWidth,
              d,
              y,
              baselineY,
            )
            const trim = Math.min(rise, Math.max(0, barHeight - 1))
            y += trim
            barHeight -= trim
          }
        }

        const isFaded = (hoveredBarIndex !== null && hoveredBarIndex !== i) || isLegendDimmed

        // Use categoryValue as key since it's the unique identifier from data
        const barKey = `bar-${dataKey}-${categoryValue}`

        // Apply rounded corners:
        // - For non-stacked: always apply
        // - For stacked with gap: apply to all bars
        // - For stacked without gap: only apply to the last series
        const applyRounding = !stacked || stackGap > 0 || isLastSeries
        const effectiveRx = applyRounding ? cornerRadius : 0
        const effectiveRy = applyRounding ? cornerRadius : 0

        if (animate && !reducedMotion && !isLoaded) {
          return (
            <AnimatedBar
              edge={edge}
              animationType={animationType}
              enterTransition={enterTransition}
              fadedOpacity={fadedOpacity}
              fill={fill}
              height={barHeight}
              index={i}
              innerHeight={innerHeight}
              isFaded={isFaded}
              isHorizontal={isHorizontal}
              key={barKey}
              revealEpoch={revealEpoch}
              rx={effectiveRx}
              ry={effectiveRy}
              staggerDelay={calculatedStaggerDelay}
              width={barW}
              x={x}
              y={y}
            />
          )
        }

        // Static bar after animation completes
        return (
          <g key={barKey}>
            <rect
              fill={fill}
              height={barHeight}
              opacity={isFaded ? fadedOpacity : 1}
              rx={effectiveRx}
              ry={effectiveRy}
              style={{
                cursor: 'default',
                transition: 'opacity 0.15s ease-in-out',
              }}
              width={barW}
              x={x}
              y={y}
            />
            {isHorizontal ? null : (
              <BarEdge
                edge={edge}
                height={barHeight}
                opacity={isFaded ? fadedOpacity : 1}
                width={barW}
                x={x}
                y={y}
              />
            )}
          </g>
        )
      })}
    </g>
  )
})

export function Bar(props: BarProps) {
  const { barScale, bandWidth, barXAccessor } = useChartStable()

  if (!(barScale && bandWidth && barXAccessor)) {
    console.warn('Bar component must be used within a BarChart')
    return null
  }

  return (
    <BarInner {...props} bandWidth={bandWidth} barScale={barScale} barXAccessor={barXAccessor} />
  )
}

Bar.displayName = 'Bar'

export default Bar
