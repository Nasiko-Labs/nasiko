/* eslint-disable react-hooks/refs, react-hooks/set-state-in-effect -- Kit animation code (from the React migration's visx kit): it mounts portals after the
   first render and drives motion springs from values tracked across renders. Correct without the React Compiler,
   so the file opts out of it with 'use no memo'. */
'use client'
'use no memo'

import { motion, useSpring } from 'motion/react'
import { memo, use, useEffect, useMemo, useState } from 'react'
import { createPortal } from 'react-dom'
import { resolveTooltipBoxMotion, type SpringConfig, useChartConfig } from '../chart-config-context'
import { chartCssVars, type LineConfig, useChart, useChartStable } from '../chart-context'
import { ChartDateFormat } from '../chart-formatters'
import type { IndicatorFadeEdges } from '../indicator-fade'
import { DateTicker } from './date-ticker'
import { TooltipBox } from './tooltip-box'
import { TooltipContent, type TooltipRow } from './tooltip-content'
import { TooltipDot } from './tooltip-dot'
import { TooltipIndicator } from './tooltip-indicator'

export interface ChartTooltipProps {
  /** Whether to show the date pill at bottom. Default: true */
  showDatePill?: boolean
  /** Whether to show the vertical crosshair line. Default: true */
  showCrosshair?: boolean
  /** Whether to show dots on the lines. Default: true */
  showDots?: boolean
  /** Dot style: filled circle or transparent ring. Default: "dot" */
  dotVariant?: 'dot' | 'ring'
  /** Dot / ring radius in pixels. Default: 5 */
  dotSize?: number
  /** Ring corner radius as a fraction of side length (0 = square, 0.5 = circle). */
  dotRadiusFraction?: number
  /** Multiplier applied to the computed dot / ring pixel radius. Default: 1 */
  dotScale?: number
  /** Ring stroke width in pixels. Default: 1.5 for ring variant */
  dotStrokeWidth?: number
  /**
   * Color for the crosshair/indicator line. When a function, receives the hovered point
   * (e.g. for candlestick: match candle color from close vs open). Default: --chart-crosshair.
   */
  indicatorColor?: string | ((point: Record<string, unknown>) => string)
  /** Custom content renderer for the tooltip box */
  content?: (props: { point: Record<string, unknown>; index: number }) => React.ReactNode
  /** Custom row renderer - return array of TooltipRow */
  rows?: (point: Record<string, unknown>) => TooltipRow[]
  /**
   * Override tooltip dot fill. When omitted and `rows` is set, dot colors match row colors.
   * When a function, receives the hovered point and line config.
   */
  dotColor?: string | ((point: Record<string, unknown>, line: LineConfig) => string)
  /** Additional content to show below rows (e.g., markers) */
  children?: React.ReactNode
  /** Custom class name */
  className?: string
  /** Per-chart override for the crosshair / dot / date-pill spring. */
  springConfig?: SpringConfig
  /**
   * When `true`, the floating panel uses the crosshair spring and stays in sync.
   * Default `false` — panel follow uses `damping` (`20`).
   */
  matchCrosshair?: boolean
  /**
   * Spring damping for the floating tooltip panel when `matchCrosshair` is `false`.
   * `0` disables spring motion (instant). Default: `20`.
   */
  damping?: number
  /** SVG stroke dash pattern for the crosshair. Omit for solid. */
  indicatorDasharray?: string
  /** Vertical crosshair fade: `both`, `top`, `bottom`, or `none` (solid). Default: `both`. */
  indicatorFadeEdges?: IndicatorFadeEdges
  /** Crosshair fade zone size (% of height). Default: `10`. */
  indicatorFadeLength?: number
  /** Per-chart override for the floating-panel spring. */
  boxSpringConfig?: SpringConfig
  /** Inline styles for the tooltip panel (background, blur, etc.). */
  panelStyle?: React.CSSProperties
  /**
   * Tooltip panel background color (CSS variable or color value).
   * Default: `var(--chart-tooltip-background)`.
   */
  backgroundColor?: string
}

interface ChartTooltipInnerProps extends ChartTooltipProps {
  container: HTMLElement
}

const ChartTooltipInner = memo(function ChartTooltipInner({
  showDatePill = true,
  showCrosshair = true,
  showDots = true,
  dotVariant = 'dot',
  dotSize = 5,
  dotRadiusFraction,
  dotScale = 1,
  dotStrokeWidth,
  indicatorColor: indicatorColorProp,
  content,
  rows: rowsRenderer,
  dotColor: dotColorProp,
  children,
  className = '',
  container,
  springConfig,
  matchCrosshair = false,
  damping,
  indicatorDasharray,
  indicatorFadeEdges,
  indicatorFadeLength,
  boxSpringConfig,
  panelStyle,
  backgroundColor,
}: ChartTooltipInnerProps) {
  const {
    tooltipData,
    width,
    height,
    innerHeight,
    margin,
    columnWidth,
    lines,
    xAccessor,
    dateLabels,
    containerRef,
    orientation,
    barXAccessor,
    bandWidth,
    squareSnap,
  } = useChart()
  const { tooltipSpring } = useChartConfig()

  const isHorizontal = orientation === 'horizontal'
  const discreteInteraction = dateLabels.length > 60

  const resolvedDotSize = useMemo(() => {
    if (dotVariant !== 'ring' || !bandWidth || lines.length === 0) {
      return dotSize * dotScale
    }
    const seriesCount = lines.length
    const gap = squareSnap?.groupGap ?? (seriesCount > 1 ? 4 : 0)
    const squareSize = (bandWidth - gap * (seriesCount - 1)) / seriesCount
    return (squareSize / 2) * dotScale
  }, [bandWidth, dotScale, dotSize, dotVariant, lines.length, squareSnap?.groupGap])
  const boxMotion = useMemo(() => {
    if (boxSpringConfig) {
      return {
        animate: !discreteInteraction,
        springConfig: boxSpringConfig,
      }
    }
    if (matchCrosshair) {
      return {
        animate: !discreteInteraction,
        springConfig: springConfig ?? tooltipSpring,
      }
    }
    return resolveTooltipBoxMotion(damping)
  }, [boxSpringConfig, damping, discreteInteraction, matchCrosshair, springConfig, tooltipSpring])

  const visible = tooltipData !== null
  const x = tooltipData?.x ?? 0
  const xWithMargin = x + margin.left

  // For horizontal charts, get the y position from the first line's yPosition (center of bar)
  const firstLineDataKey = lines[0]?.dataKey
  const firstLineY = firstLineDataKey ? (tooltipData?.yPositions[firstLineDataKey] ?? 0) : 0
  const yWithMargin = firstLineY + margin.top

  const tooltipRows = useMemo(() => {
    if (!tooltipData) {
      return []
    }

    if (rowsRenderer) {
      return rowsRenderer(tooltipData.point)
    }

    // Default: generate rows from registered lines
    return lines.map((line) => ({
      color: line.stroke,
      label: line.dataKey,
      value: (tooltipData.point[line.dataKey] as number) ?? 0,
    }))
  }, [tooltipData, lines, rowsRenderer])

  const resolveDotColor = useMemo(() => {
    return (line: LineConfig, index: number): string => {
      if (rowsRenderer && tooltipRows[index]?.color) {
        return tooltipRows[index].color
      }
      if (dotColorProp != null) {
        if (typeof dotColorProp === 'function' && tooltipData) {
          return dotColorProp(tooltipData.point, line)
        }
        if (typeof dotColorProp === 'string') {
          return dotColorProp
        }
      }
      return line.stroke
    }
  }, [dotColorProp, rowsRenderer, tooltipData, tooltipRows])

  // Resolve indicator color (static or from hovered point)
  const indicatorColor = useMemo(() => {
    if (indicatorColorProp == null) {
      return chartCssVars.crosshair
    }
    if (typeof indicatorColorProp === 'function') {
      return tooltipData ? indicatorColorProp(tooltipData.point) : chartCssVars.crosshair
    }
    return indicatorColorProp
  }, [indicatorColorProp, tooltipData])

  // Lab: the title uses the chart's ChartDateFormat (UTC here), like the axis; the kit used a local weekday format.
  const formatDate = use(ChartDateFormat)
  // Title from date or category
  const title = useMemo(() => {
    if (!tooltipData) {
      return undefined
    }
    // For bar charts (horizontal or vertical), use the category name
    if (barXAccessor) {
      return barXAccessor(tooltipData.point)
    }
    // For line/area charts, use the date
    return formatDate(xAccessor(tooltipData.point))
  }, [tooltipData, barXAccessor, xAccessor, formatDate])

  const tooltipContent = (
    <>
      {/* Crosshair indicator - rendered as SVG overlay */}
      {showCrosshair && (
        <svg
          aria-hidden="true"
          className="pointer-events-none absolute inset-0"
          height="100%"
          width="100%"
        >
          <g transform={`translate(${margin.left},${margin.top})`}>
            <TooltipIndicator
              animate={!discreteInteraction}
              colorEdge={indicatorColor}
              colorMid={indicatorColor}
              columnWidth={columnWidth}
              fadeEdges={indicatorDasharray ? 'none' : (indicatorFadeEdges ?? 'both')}
              fadeLength={indicatorFadeLength}
              height={innerHeight}
              springConfig={springConfig}
              strokeDasharray={indicatorDasharray}
              visible={visible}
              width="line"
              x={x}
            />
          </g>
        </svg>
      )}

      {/* Dots on bars/lines - show for vertical charts only */}
      {showDots && visible && !isHorizontal && (
        <svg
          aria-hidden="true"
          className="pointer-events-none absolute inset-0"
          height="100%"
          width="100%"
        >
          <g transform={`translate(${margin.left},${margin.top})`}>
            {lines.map((line, index) => (
              <TooltipDot
                color={resolveDotColor(line, index)}
                cornerRadiusFraction={dotVariant === 'ring' ? dotRadiusFraction : undefined}
                key={line.dataKey}
                size={resolvedDotSize}
                springConfig={springConfig}
                strokeColor={chartCssVars.background}
                strokeWidth={dotVariant === 'ring' ? dotStrokeWidth : undefined}
                variant={dotVariant}
                visible={visible}
                x={tooltipData?.xPositions?.[line.dataKey] ?? x}
                y={tooltipData?.yPositions[line.dataKey] ?? 0}
              />
            ))}
          </g>
        </svg>
      )}

      {/* Tooltip Box */}
      <TooltipBox
        animate={boxMotion.animate}
        backgroundColor={backgroundColor}
        className={className}
        containerHeight={height}
        containerRef={containerRef}
        containerWidth={width}
        panelStyle={panelStyle}
        springConfig={boxMotion.springConfig}
        top={isHorizontal ? undefined : margin.top}
        visible={visible}
        x={xWithMargin}
        y={isHorizontal ? yWithMargin : margin.top}
      >
        {content && tooltipData
          ? content({
              point: tooltipData.point,
              index: tooltipData.index,
            })
          : !content && (
              <TooltipContent rows={tooltipRows} title={title}>
                {children}
              </TooltipContent>
            )}
      </TooltipBox>

      {/* Date/Category Ticker - only show for vertical charts */}
      <DatePillTracker
        currentIndex={tooltipData?.index ?? 0}
        discreteInteraction={discreteInteraction}
        enabled={showDatePill && !isHorizontal}
        labels={dateLabels}
        springConfig={springConfig}
        visible={visible}
        xWithMargin={xWithMargin}
      />
    </>
  )

  return createPortal(tooltipContent, container)
})

export function ChartTooltip(props: ChartTooltipProps) {
  const { containerRef } = useChartStable()
  const [mounted, setMounted] = useState(false)

  // Only render portals on client side after mount
  useEffect(() => {
    setMounted(true)
  }, [])

  const container = containerRef.current
  if (!(mounted && container)) {
    return null
  }

  return <ChartTooltipInner {...props} container={container} />
}

ChartTooltip.displayName = 'ChartTooltip'

interface DatePillTrackerProps {
  enabled: boolean
  visible: boolean
  labels: string[]
  currentIndex: number
  xWithMargin: number
  discreteInteraction: boolean
  springConfig?: SpringConfig
}

// Inner-only-on-visible so `useSpring` initializes at the real cursor x
// instead of `margin.left` on first hover.
function DatePillTracker(props: DatePillTrackerProps) {
  if (!(props.enabled && props.visible && props.labels.length > 0)) {
    return null
  }
  return <DatePillTrackerInner {...props} />
}

function DatePillTrackerInner({
  labels,
  currentIndex,
  xWithMargin,
  discreteInteraction,
  springConfig,
  visible,
}: DatePillTrackerProps) {
  const { tooltipSpring } = useChartConfig()
  const effectiveSpring = springConfig ?? tooltipSpring
  const animatedX = useSpring(xWithMargin, effectiveSpring)

  if (!discreteInteraction) {
    animatedX.set(xWithMargin)
  }

  useEffect(() => {
    animatedX.set(xWithMargin)
    // eslint-disable-next-line react-hooks/exhaustive-deps -- jump animatedX only when `visible` changes, not on every move
  }, [animatedX, visible])

  return (
    <motion.div
      className="pointer-events-none absolute z-50"
      style={{
        left: discreteInteraction ? xWithMargin : animatedX,
        transform: 'translateX(-50%)',
        bottom: 4,
      }}
    >
      <DateTicker currentIndex={currentIndex} labels={labels} visible={visible} />
    </motion.div>
  )
}

export default ChartTooltip
