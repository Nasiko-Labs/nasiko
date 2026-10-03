'use client'

import { motion, useSpring } from 'motion/react'
import { useEffect } from 'react'
import { type SpringConfig, useChartConfig } from '../chart-config-context'
import { chartCssVars } from '../chart-context'
import {
  type IndicatorFadeEdges,
  indicatorFadeGradientStops,
  resolveVerticalFadeSides,
} from '../indicator-fade'

export type IndicatorWidth =
  | number // Pixel width
  | 'line' // 1px line (default)
  | 'thin' // 2px
  | 'medium' // 4px
  | 'thick' // 8px

export interface TooltipIndicatorProps {
  /** X position in pixels (center of the indicator) */
  x: number
  /** Height of the indicator */
  height: number
  /** Whether the indicator is visible */
  visible: boolean
  /**
   * Width of the indicator - number (pixels) or preset.
   * Ignored if `span` is provided.
   */
  width?: IndicatorWidth
  /**
   * Number of columns/days to span, with current point centered.
   * Requires `columnWidth` to be set.
   */
  span?: number
  /** Width of a single column/day in pixels. Required when using `span`. */
  columnWidth?: number
  /** Primary color at edges (10% and 90%) */
  colorEdge?: string
  /** Secondary color at center (50%) */
  colorMid?: string
  /** Vertical fade: both ends, top, bottom, or none (solid). */
  fadeEdges?: IndicatorFadeEdges | boolean
  /** Fade zone size as a percentage of indicator height. Default: 10 */
  fadeLength?: number
  /** Animate position with a spring. Default: true */
  animate?: boolean
  /** Unique ID for the gradient */
  gradientId?: string
  /** Per-chart override; falls back to `ChartConfigProvider.tooltipSpring`. */
  springConfig?: SpringConfig
  /** SVG stroke dash pattern. When set, renders a dashed stroke instead of a solid fill. */
  strokeDasharray?: string
}

function resolveWidth(width: IndicatorWidth): number {
  if (typeof width === 'number') {
    return width
  }
  switch (width) {
    case 'line':
      return 1
    case 'thin':
      return 2
    case 'medium':
      return 4
    case 'thick':
      return 8
    default:
      return 1
  }
}

// Inner-only-on-visible so `useSpring` initializes at the real cursor x
// instead of 0 on first hover.
export function TooltipIndicator(props: TooltipIndicatorProps) {
  if (!props.visible) {
    return null
  }
  return <TooltipIndicatorInner {...props} />
}

function TooltipIndicatorInner({
  x,
  visible,
  height,
  width = 'line',
  span,
  columnWidth,
  colorEdge = chartCssVars.crosshair,
  colorMid = chartCssVars.crosshair,
  fadeEdges = 'both',
  fadeLength = 10,
  animate = true,
  gradientId = 'tooltip-indicator-gradient',
  springConfig,
  strokeDasharray,
}: TooltipIndicatorProps) {
  const { tooltipSpring } = useChartConfig()
  const effectiveSpring = springConfig ?? tooltipSpring

  const pixelWidth =
    span !== undefined && columnWidth !== undefined ? span * columnWidth : resolveWidth(width)

  const rectX = x - pixelWidth / 2
  const lineX = x
  const animatedX = useSpring(rectX, effectiveSpring)
  const animatedLineX = useSpring(lineX, effectiveSpring)

  if (animate) {
    animatedX.set(rectX)
    animatedLineX.set(lineX)
  }

  // we need to jump the animatedX when the visible prop changes
  useEffect(() => {
    animatedX.set(rectX)
    animatedLineX.set(lineX)
  }, [animatedLineX, animatedX, lineX, rectX, visible])

  const indicatorFill = colorMid || colorEdge
  const fadeSides = resolveVerticalFadeSides(fadeEdges)
  const dashed = Boolean(strokeDasharray)

  if (dashed) {
    const strokeWidth = Math.max(1, pixelWidth)
    return animate ? (
      <motion.line
        stroke={indicatorFill}
        strokeDasharray={strokeDasharray}
        strokeWidth={strokeWidth}
        x1={animatedLineX}
        x2={animatedLineX}
        y1={0}
        y2={height}
      />
    ) : (
      <line
        stroke={indicatorFill}
        strokeDasharray={strokeDasharray}
        strokeWidth={strokeWidth}
        x1={lineX}
        x2={lineX}
        y1={0}
        y2={height}
      />
    )
  }

  if (!fadeSides.any) {
    return animate ? (
      <motion.rect fill={indicatorFill} height={height} width={pixelWidth} x={animatedX} y={0} />
    ) : (
      <rect fill={indicatorFill} height={height} width={pixelWidth} x={rectX} y={0} />
    )
  }

  const fadeStops = indicatorFadeGradientStops(fadeSides, fadeLength)

  return (
    <g>
      <defs>
        <linearGradient id={gradientId} x1="0%" x2="0%" y1="0%" y2="100%">
          {fadeStops.map((stop) => (
            <stop
              key={stop.offset}
              offset={stop.offset}
              style={{ stopColor: indicatorFill, stopOpacity: stop.opacity }}
            />
          ))}
        </linearGradient>
      </defs>
      {animate ? (
        <motion.rect
          fill={`url(#${gradientId})`}
          height={height}
          width={pixelWidth}
          x={animatedX}
          y={0}
        />
      ) : (
        <rect fill={`url(#${gradientId})`} height={height} width={pixelWidth} x={rectX} y={0} />
      )}
    </g>
  )
}

TooltipIndicator.displayName = 'TooltipIndicator'

export default TooltipIndicator
