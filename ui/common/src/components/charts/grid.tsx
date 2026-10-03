'use client'

import { GridColumns, GridRows } from '@visx/grid'
import { motion } from 'motion/react'
import { useId } from 'react'
import { chartCssVars, useChartStable, useYScale } from './chart-context'
import { useGridShimmer } from './use-grid-shimmer'
import { isLoadingChromePhase, isLoadingGridChromePhase } from './y-domain-utils'

const DEFAULT_SHIMMER_LENGTH_PX = 140
const DEFAULT_SHIMMER_SPEED = 1
const DEFAULT_SHIMMER_STROKE = 'color-mix(in oklch, var(--foreground) 68%, transparent)'

export interface GridProps {
  /** Show horizontal grid lines. Default: true */
  horizontal?: boolean
  /** Show vertical grid lines. Default: false */
  vertical?: boolean
  /** Number of horizontal grid lines. Default: 5 */
  numTicksRows?: number
  /** Number of vertical grid lines. Default: 10 */
  numTicksColumns?: number
  /** Explicit tick values for horizontal grid lines. Overrides numTicksRows. */
  rowTickValues?: number[]
  /** Grid line stroke color. Default: var(--chart-grid) */
  stroke?: string
  /** Grid stroke while loading chrome is active. Falls back to `stroke`. */
  loadingStroke?: string
  /** Grid line stroke opacity. Default: 1 */
  strokeOpacity?: number
  /** Grid line stroke width. Default: 1 */
  strokeWidth?: number
  /** Grid line dash array. Default: "4,4" for dashed lines */
  strokeDasharray?: string
  /** Horizontal row values rendered with alternate styling (e.g. zero baseline). */
  highlightRowValues?: number[]
  /** Stroke for highlighted rows. Default: var(--chart-foreground-muted) */
  highlightRowStroke?: string
  /** Stroke opacity for highlighted rows. Default: 1 */
  highlightRowStrokeOpacity?: number
  /** Stroke width for highlighted rows. Default: 1 */
  highlightRowStrokeWidth?: number
  /** Dash array for highlighted rows. Default: solid line */
  highlightRowStrokeDasharray?: string
  /** Enable horizontal fade effect on grid rows (fades at left/right). Default: true */
  fadeHorizontal?: boolean
  /** Enable vertical fade effect on grid columns (fades at top/bottom). Default: false */
  fadeVertical?: boolean
  /** Omit the first and last horizontal grid lines. Default: false */
  hideHorizontalEdgeLines?: boolean
  /** Omit the first and last vertical grid lines. Default: false */
  hideVerticalEdgeLines?: boolean
  /** Y-scale for horizontal grid lines. Default: primary (`"left"`) axis. */
  yAxisId?: string | number
  /** Animate a shimmer band across horizontal grid lines. Default: false */
  shimmer?: boolean
  /** Shimmer band stroke (color and opacity via color-mix or oklch alpha). */
  shimmerStroke?: string
  /** Shimmer band width in pixels. Default: 140 */
  shimmerLength?: number
  /** Shimmer speed multiplier (higher = faster). Default: 1 */
  shimmerSpeed?: number
  /** Match loop timing to the loading line pulse (cycle + inter-loop pause). */
  shimmerSync?: boolean
}

function hideEdgeTicks<T>(ticks: T[], hideEdgeLines: boolean): T[] {
  if (!hideEdgeLines || ticks.length <= 2) {
    return ticks
  }
  return ticks.slice(1, -1)
}

function resolveRowTickValues(options: {
  hideHorizontalEdgeLines: boolean
  numTicksRows: number
  rowTickValues?: number[]
  yScale: { ticks?: (count: number) => number[] }
}): number[] | undefined {
  const { hideHorizontalEdgeLines, numTicksRows, rowTickValues, yScale } = options
  const ticks = rowTickValues ?? (yScale.ticks ? yScale.ticks(numTicksRows) : [])
  const filtered = hideEdgeTicks(ticks, hideHorizontalEdgeLines)
  if (filtered === ticks && !rowTickValues && !hideHorizontalEdgeLines) {
    return undefined
  }
  return filtered.length > 0 ? filtered : undefined
}

// Note: grid fade masks and shimmer share one layer tree
export function Grid({
  horizontal = true,
  vertical = false,
  numTicksRows = 5,
  numTicksColumns = 10,
  rowTickValues,
  stroke = chartCssVars.grid,
  loadingStroke,
  strokeOpacity = 1,
  strokeWidth = 1,
  strokeDasharray = '4,4',
  highlightRowValues,
  highlightRowStroke = chartCssVars.foregroundMuted,
  highlightRowStrokeOpacity = 1,
  highlightRowStrokeWidth = 1,
  highlightRowStrokeDasharray = '0',
  fadeHorizontal = true,
  fadeVertical = false,
  hideHorizontalEdgeLines = false,
  hideVerticalEdgeLines = false,
  yAxisId,
  shimmer = false,
  shimmerStroke = DEFAULT_SHIMMER_STROKE,
  shimmerLength = DEFAULT_SHIMMER_LENGTH_PX,
  shimmerSpeed = DEFAULT_SHIMMER_SPEED,
  shimmerSync = false,
}: GridProps) {
  const { xScale, innerWidth, innerHeight, orientation, barScale, chartPhase } = useChartStable()
  const yScale = useYScale(yAxisId)
  const shimmerActive = shimmer && isLoadingChromePhase(chartPhase)
  const gridStroke =
    isLoadingGridChromePhase(chartPhase) && loadingStroke != null ? loadingStroke : stroke
  const { shimmerEnabled, shimmerTransform } = useGridShimmer({
    innerWidth,
    shimmer,
    shimmerLength,
    shimmerSpeed,
    shimmerSync,
    active: shimmerActive,
  })

  // For bar charts, determine which scale to use for grid lines
  // Horizontal bar charts: vertical grid should use yScale (value scale)
  // Vertical bar charts: horizontal grid uses yScale (value scale)
  const isHorizontalBarChart = orientation === 'horizontal' && barScale

  // For vertical grid lines in horizontal bar charts, use yScale (the value scale)
  // For time-based charts, use xScale
  const columnScale = isHorizontalBarChart ? yScale : xScale
  const rowTickValuesResolved = resolveRowTickValues({
    hideHorizontalEdgeLines,
    numTicksRows,
    rowTickValues,
    yScale,
  })
  const columnTickValuesResolved =
    vertical && columnScale && typeof columnScale === 'function' && hideVerticalEdgeLines
      ? (() => {
          const ticks = columnScale.ticks?.(numTicksColumns) ?? []
          const filtered = hideEdgeTicks<number | Date>(ticks, true)
          return filtered.length > 0 ? filtered : undefined
        })()
      : undefined
  const uniqueId = useId()

  // Horizontal fade mask (for grid rows - fades left/right)
  const hMaskId = `grid-rows-fade-${uniqueId}`
  const hGradientId = `${hMaskId}-gradient`
  const shimmerGradientId = `grid-shimmer-${uniqueId}`

  // Vertical fade mask (for grid columns - fades top/bottom)
  const vMaskId = `grid-cols-fade-${uniqueId}`
  const vGradientId = `${vMaskId}-gradient`
  const horizontalFadeMask = fadeHorizontal ? `url(#${hMaskId})` : undefined

  return (
    <g className="chart-grid">
      {/* Gradient mask for horizontal grid lines - fades at left/right */}
      {horizontal && fadeHorizontal && (
        <defs>
          <linearGradient id={hGradientId} x1="0%" x2="100%" y1="0%" y2="0%">
            <stop offset="0%" style={{ stopColor: 'white', stopOpacity: 0 }} />
            <stop offset="10%" style={{ stopColor: 'white', stopOpacity: 1 }} />
            <stop offset="90%" style={{ stopColor: 'white', stopOpacity: 1 }} />
            <stop offset="100%" style={{ stopColor: 'white', stopOpacity: 0 }} />
          </linearGradient>
          <mask id={hMaskId}>
            <rect
              fill={`url(#${hGradientId})`}
              height={innerHeight}
              width={innerWidth}
              x="0"
              y="0"
            />
          </mask>
        </defs>
      )}

      {horizontal && shimmerEnabled ? (
        <defs>
          <motion.linearGradient
            gradientTransform={shimmerTransform}
            gradientUnits="userSpaceOnUse"
            id={shimmerGradientId}
            x1={0}
            x2={shimmerLength}
            y1={0}
            y2={0}
          >
            <stop offset="0%" stopColor={shimmerStroke} stopOpacity={0} />
            <stop offset="35%" stopColor={shimmerStroke} stopOpacity={0.45} />
            <stop offset="50%" stopColor={shimmerStroke} stopOpacity={1} />
            <stop offset="65%" stopColor={shimmerStroke} stopOpacity={0.45} />
            <stop offset="100%" stopColor={shimmerStroke} stopOpacity={0} />
          </motion.linearGradient>
        </defs>
      ) : null}

      {/* Gradient mask for vertical grid lines - fades at top/bottom */}
      {vertical && fadeVertical && (
        <defs>
          <linearGradient id={vGradientId} x1="0%" x2="0%" y1="0%" y2="100%">
            <stop offset="0%" style={{ stopColor: 'white', stopOpacity: 0 }} />
            <stop offset="10%" style={{ stopColor: 'white', stopOpacity: 1 }} />
            <stop offset="90%" style={{ stopColor: 'white', stopOpacity: 1 }} />
            <stop offset="100%" style={{ stopColor: 'white', stopOpacity: 0 }} />
          </linearGradient>
          <mask id={vMaskId}>
            <rect
              fill={`url(#${vGradientId})`}
              height={innerHeight}
              width={innerWidth}
              x="0"
              y="0"
            />
          </mask>
        </defs>
      )}

      {horizontal && (
        <g mask={horizontalFadeMask}>
          <GridRows
            numTicks={rowTickValuesResolved ? undefined : numTicksRows}
            scale={yScale}
            stroke={gridStroke}
            strokeDasharray={strokeDasharray}
            strokeOpacity={strokeOpacity}
            strokeWidth={strokeWidth}
            tickValues={rowTickValuesResolved}
            width={innerWidth}
          />
          {shimmerEnabled ? (
            <GridRows
              numTicks={rowTickValuesResolved ? undefined : numTicksRows}
              scale={yScale}
              stroke={`url(#${shimmerGradientId})`}
              strokeDasharray={strokeDasharray}
              strokeOpacity={1}
              strokeWidth={strokeWidth}
              tickValues={rowTickValuesResolved}
              width={innerWidth}
            />
          ) : null}
        </g>
      )}
      {horizontal && highlightRowValues && highlightRowValues.length > 0 ? (
        <g className="chart-grid-highlight-rows">
          {highlightRowValues.map((value) => {
            const y = yScale(value)
            if (y == null || !Number.isFinite(y)) {
              return null
            }

            return (
              <line
                key={value}
                stroke={highlightRowStroke}
                strokeDasharray={highlightRowStrokeDasharray}
                strokeOpacity={highlightRowStrokeOpacity}
                strokeWidth={highlightRowStrokeWidth}
                x1={0}
                x2={innerWidth}
                y1={y}
                y2={y}
              />
            )
          })}
        </g>
      ) : null}
      {vertical && columnScale && typeof columnScale === 'function' && (
        <g mask={fadeVertical ? `url(#${vMaskId})` : undefined}>
          <GridColumns
            height={innerHeight}
            numTicks={columnTickValuesResolved ? undefined : numTicksColumns}
            scale={columnScale}
            stroke={stroke}
            strokeDasharray={strokeDasharray}
            strokeOpacity={strokeOpacity}
            strokeWidth={strokeWidth}
            tickValues={columnTickValuesResolved}
          />
        </g>
      )}
    </g>
  )
}

Grid.displayName = 'Grid'

export default Grid
