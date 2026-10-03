'use client'

import { localPoint } from '@visx/event'
import { ParentSize } from '@visx/responsive'
import { scaleBand, scaleLinear } from '@visx/scale'
import type { Transition } from 'motion/react'
import {
  memo,
  type ReactElement,
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import { cn } from '@/lib/utils'
import { DEFAULT_ANIMATION_EASING } from './animation'
import type { BarProps } from './bar'
import { topSquareCenterY } from './bar-squares-layout'
import {
  forEachChartChild,
  isChartClipPassthrough,
  isClipExcludedComponent,
  isPostOverlayComponent,
  isUnderlayComponent,
  renderKeyedChartLayers,
  resolveChartChildElement,
} from './chart-child-passthrough'
import { type LineConfig, type Margin, type TooltipData } from './chart-context'
import { ChartProvider } from './chart-provider'
import { isGradientDefComponent, isPatternDefComponent } from './chart-defs'
import { shortDateFmt } from './chart-formatters'
import {
  type ChartPhase,
  type ChartStatus,
  DEFAULT_CHART_LIFECYCLE,
  resolveRestingChartPhase,
} from './chart-phase'
import { BarLoadingSkeleton } from './loading-sweep'
import { extractReferenceAreaConfigs } from './reference-area-config'
import { useScheduledTooltip } from './use-scheduled-tooltip'
import {
  buildYScalesForLines,
  getPrimaryYScale,
  normalizeYAxisId,
  wrapSingleYScale,
} from './y-axis-scales'

/** Skeleton bars to show when `status="loading"` and `data` is empty. */
const FALLBACK_LOADING_BARS = 12

export type BarOrientation = 'vertical' | 'horizontal'

export interface BarChartProps {
  /** Data array - each item should have an x-axis key and numeric values */
  data: Record<string, unknown>[]
  /** Key in data for the categorical axis. Default: "name" */
  xDataKey?: string
  /** Chart margins */
  margin?: Partial<Margin>
  /** Animation duration in milliseconds. Default: 1100 */
  animationDuration?: number
  /** CSS easing for bar grow transitions. */
  animationEasing?: string
  /** Motion enter transition (spring or cubic-bezier tween). */
  enterTransition?: Transition
  /** Signature of motion URL state — triggers enter replay when it changes. */
  revealSignature?: string
  /** Aspect ratio as "width / height". Default: "2 / 1" */
  aspectRatio?: string
  /** Additional class name for the container */
  className?: string
  /** Gap between bar groups as a fraction of band width (0-1). Default: 0.2 */
  barGap?: number
  /** Fixed bar width in pixels. If not set, bars auto-size to fill the band. */
  barWidth?: number
  /** Bar chart orientation. Default: "vertical" */
  orientation?: BarOrientation
  /** Whether to stack bars instead of grouping them. Default: false */
  stacked?: boolean
  /** Gap between stacked bar segments in pixels. Default: 0 */
  stackGap?: number
  /** When set, tooltip Y positions snap to the top square center (shape variant). */
  squareSnap?: { squareGap: number; groupGap?: number; fit?: boolean }
  /** Child components (Bar, Grid, ChartTooltip, etc.). Optional — omit for a
   * pure `status="loading"` skeleton. */
  children?: ReactNode
  /** Reports reveal lifecycle for OG screenshots and loading orchestration. */
  onPhaseChange?: (phase: ChartPhase) => void
  /** Fetch / display status. When `"loading"`, a shimmer skeleton replaces the
   * bars (no chart data required). Default: `"ready"`. */
  status?: ChartStatus
}

const FAKE_TIME_EPOCH = Date.UTC(2000, 0, 1)

const DEFAULT_MARGIN: Margin = { top: 40, right: 40, bottom: 40, left: 40 }

// Extract bar configs from children synchronously
function extractBarConfigs(children: ReactNode): LineConfig[] {
  const configs: LineConfig[] = []

  forEachChartChild(children, (child) => {
    const childType = child.type as {
      displayName?: string
      name?: string
      __isBarDepthLayer?: boolean
    }
    // Bar-depth surface layers (BarDepthBack/Front, BarPulse) carry a
    // `dataKey` to pair with a Bar but are not series themselves — skip them
    // so they don't inflate the series count and shrink the real bars.
    if (childType.__isBarDepthLayer) {
      return
    }
    const componentName =
      typeof child.type === 'function' ? childType.displayName || childType.name || '' : ''

    const props = child.props as BarProps | undefined
    const isBarComponent =
      componentName === 'Bar' ||
      componentName === 'BarSquares' ||
      (props && typeof props.dataKey === 'string' && props.dataKey.length > 0)

    if (isBarComponent && props?.dataKey) {
      // Use stroke for tooltip dot color if provided, otherwise fall back to fill
      // This allows gradient/pattern fills to have a solid dot color
      const dotColor = props.stroke || props.fill || 'var(--chart-line-primary)'
      configs.push({
        dataKey: props.dataKey,
        stroke: dotColor,
        strokeWidth: 0,
        yAxisId: props.yAxisId,
      })
    }
  })

  return configs
}

interface ChartInnerProps {
  width: number
  height: number
  data: Record<string, unknown>[]
  xDataKey: string
  margin: Margin
  animationDuration: number
  animationEasing: string
  enterTransition?: Transition
  revealSignature?: string
  barGap: number
  barWidthProp?: number
  orientation: BarOrientation
  stacked: boolean
  stackGap: number
  squareSnap?: { squareGap: number; groupGap?: number; fit?: boolean }
  children: ReactNode
  containerRef: React.RefObject<HTMLDivElement | null>
  onPhaseChange?: (phase: ChartPhase) => void
  status: ChartStatus
}

function ChartInner(props: ChartInnerProps) {
  const { width, height } = props
  if (width < 10 || height < 10) {
    return null
  }
  return <ChartCore {...props} />
}

const ChartCore = memo(function ChartCore({
  width,
  height,
  data,
  xDataKey,
  margin,
  animationDuration,
  animationEasing,
  enterTransition,
  revealSignature = '',
  barGap,
  barWidthProp,
  orientation,
  stacked,
  stackGap,
  squareSnap,
  children,
  containerRef,
  onPhaseChange,
  status,
}: ChartInnerProps) {
  const { tooltipData, setTooltipData, scheduleTooltip, clearTooltip } =
    useScheduledTooltip<TooltipData>()
  const [isLoaded, setIsLoaded] = useState(false)
  const [revealEpoch, setRevealEpoch] = useState(0)
  const hoveredBarIndex = tooltipData?.index ?? null

  const isHorizontal = orientation === 'horizontal'

  // Extract bar configs synchronously from children
  const lines = useMemo(() => extractBarConfigs(children), [children])

  const innerWidth = width - margin.left - margin.right
  const innerHeight = height - margin.top - margin.bottom

  // Category accessor function - returns string for categorical scale
  const categoryAccessor = useCallback(
    (d: Record<string, unknown>): string => {
      const value = d[xDataKey]
      if (value instanceof Date) {
        return shortDateFmt.format(value)
      }
      return String(value ?? '')
    },
    [xDataKey],
  )

  // For compatibility with ChartContext, provide a Date-based xAccessor
  const xAccessorDate = useCallback(
    (d: Record<string, unknown>): Date => {
      const value = d[xDataKey]
      if (value instanceof Date) {
        return value
      }
      return new Date()
    },
    [xDataKey],
  )

  // Category scale (band) - for the categorical axis
  const categoryScale = useMemo(() => {
    const domain = data.map((d) => categoryAccessor(d))
    const range: [number, number] = isHorizontal ? [0, innerHeight] : [0, innerWidth]
    return scaleBand<string>({
      range,
      domain,
      padding: barGap,
    })
  }, [innerWidth, innerHeight, data, categoryAccessor, barGap, isHorizontal])

  // Band width for bars - use prop if provided, otherwise use scale's bandwidth
  const bandWidth = barWidthProp ?? categoryScale.bandwidth()

  // Compute max value considering stacking
  const maxValue = useMemo(() => {
    if (stacked) {
      // For stacked bars, sum all values at each data point
      let max = 0
      for (const d of data) {
        let sum = 0
        for (const line of lines) {
          const value = d[line.dataKey]
          if (typeof value === 'number') {
            sum += value
          }
        }
        if (sum > max) {
          max = sum
        }
      }
      return max || 100
    }
    // For grouped bars, find max single value
    let max = 0
    for (const line of lines) {
      for (const d of data) {
        const value = d[line.dataKey]
        if (typeof value === 'number' && value > max) {
          max = value
        }
      }
    }
    return max || 100
  }, [data, lines, stacked])

  // Value scale (linear) - for the value axis
  const valueScale = useMemo(() => {
    const range = isHorizontal ? [0, innerWidth] : [innerHeight, 0]
    return scaleLinear({
      range,
      domain: [0, maxValue * 1.1],
      nice: true,
    })
  }, [innerWidth, innerHeight, maxValue, isHorizontal])

  const yScales = useMemo(() => {
    if (isHorizontal) {
      return wrapSingleYScale(valueScale)
    }
    return buildYScalesForLines({
      lines,
      data,
      innerHeight,
      resolveDomain: (dataKeys) => {
        // Stacked bars reach the sum of their series, not the largest one.
        let max = 0
        for (const d of data) {
          let sum = 0
          for (const key of dataKeys) {
            const value = d[key]
            if (typeof value !== 'number') continue
            if (stacked) sum += value
            else if (value > max) max = value
          }
          if (stacked && sum > max) max = sum
        }
        return [0, (max || 100) * 1.1]
      },
    })
  }, [data, innerHeight, isHorizontal, lines, stacked, valueScale])

  const primaryYScale = getPrimaryYScale(yScales, valueScale)

  // Compute stack offsets for stacked bars
  const stackOffsets = useMemo(() => {
    if (!stacked) {
      return undefined
    }
    const offsets = new Map<number, Map<string, number>>()
    for (let i = 0; i < data.length; i++) {
      const d = data[i]
      if (!d) {
        continue
      }
      const pointOffsets = new Map<string, number>()
      let cumulative = 0
      for (const line of lines) {
        pointOffsets.set(line.dataKey, cumulative)
        const value = d[line.dataKey]
        if (typeof value === 'number') {
          cumulative += value
        }
      }
      offsets.set(i, pointOffsets)
    }
    return offsets
  }, [data, lines, stacked])

  // Column width for tooltip indicator
  const columnWidth = useMemo(() => {
    if (data.length < 1) {
      return 0
    }
    return isHorizontal ? innerHeight / data.length : innerWidth / data.length
  }, [innerWidth, innerHeight, data.length, isHorizontal])

  // Pre-compute labels for ticker animation
  const dateLabels = useMemo(() => data.map((d) => categoryAccessor(d)), [data, categoryAccessor])

  // Create a fake time scale for compatibility with ChartContext
  // Lab: a fixed epoch instead of Date.now() (render must stay pure); only the span matters here.
  const fakeTimeScale = useMemo(() => {
    const now = FAKE_TIME_EPOCH
    const start = now - data.length * 24 * 60 * 60 * 1000
    const scale = {
      ...categoryScale,
      domain: () => [new Date(start), new Date(now)],
      range: () => [0, innerWidth] as [number, number],
      invert: (x: number) => new Date(start + (x / innerWidth) * (now - start)),
      copy: () => scale,
    }
    return scale
  }, [categoryScale, innerWidth, data.length])

  // Animation timing — replay when motion settings change
  useEffect(() => {
    setRevealEpoch((n) => n + 1)
    setIsLoaded(false)
    // While loading, hold the skeleton (no reveal, no interaction). When
    // status flips to "ready" this effect re-runs and plays the grow reveal.
    if (status === 'loading') {
      return
    }
    const staggerMs = data.length > 1 ? animationDuration * 0.4 : 0
    const timer = setTimeout(() => {
      setIsLoaded(true)
    }, animationDuration + staggerMs)
    return () => clearTimeout(timer)
    // eslint-disable-next-line react-hooks/exhaustive-deps -- revealSignature replays the reveal; a new data length alone must not
  }, [animationDuration, revealSignature, status])

  useEffect(() => {
    onPhaseChange?.(isLoaded ? 'ready' : 'revealing')
  }, [isLoaded, onPhaseChange])

  // Mouse move handler
  const handleMouseMove = useCallback(
    (event: React.MouseEvent<SVGGElement>) => {
      const point = localPoint(event)
      if (!point) {
        return
      }

      const pos = isHorizontal ? point.y - margin.top : point.x - margin.left

      // Find which band the mouse is over
      const bandIndex = Math.floor(pos / columnWidth)
      const clampedIndex = Math.max(0, Math.min(data.length - 1, bandIndex))
      const d = data[clampedIndex]

      if (!d) {
        return
      }

      // Calculate positions for each bar
      const yPositions: Record<string, number> = {}
      const xPositions: Record<string, number> = {}
      const barPos = categoryScale(categoryAccessor(d)) ?? 0

      if (isHorizontal) {
        // Horizontal bars: dots at end of bar (x = value), centered vertically in band
        const seriesCount = lines.length
        const groupGap = seriesCount > 1 ? 4 : 0
        const individualBarHeight =
          seriesCount > 0 ? (bandWidth - groupGap * (seriesCount - 1)) / seriesCount : bandWidth

        if (stacked) {
          // Stacked horizontal: all bars same y, x at cumulative end
          let cumulative = 0
          for (const line of lines) {
            const value = d[line.dataKey]
            if (typeof value === 'number') {
              cumulative += value
              const axisScale = yScales[normalizeYAxisId(line.yAxisId)] ?? valueScale
              xPositions[line.dataKey] = axisScale(cumulative) ?? 0
              yPositions[line.dataKey] = barPos + bandWidth / 2
            }
          }
        } else {
          // Grouped horizontal: each bar at its own y position
          lines.forEach((line, idx) => {
            const value = d[line.dataKey]
            if (typeof value === 'number') {
              const axisScale = yScales[normalizeYAxisId(line.yAxisId)] ?? valueScale
              xPositions[line.dataKey] = axisScale(value) ?? 0
              yPositions[line.dataKey] =
                barPos + idx * (individualBarHeight + groupGap) + individualBarHeight / 2
            }
          })
        }
      } else if (stacked) {
        // Vertical stacked bars
        let cumulative = 0
        let seriesIdx = 0
        for (const line of lines) {
          const value = d[line.dataKey]
          if (typeof value === 'number') {
            cumulative += value
            const axisScale = yScales[normalizeYAxisId(line.yAxisId)] ?? primaryYScale
            const gapOffset = seriesIdx * stackGap
            yPositions[line.dataKey] = (axisScale(cumulative) ?? 0) - gapOffset
            seriesIdx++
          }
        }
      } else {
        // Vertical grouped bars
        const seriesCount = lines.length
        const groupGap = seriesCount > 1 ? 4 : 0
        const individualBarWidth =
          seriesCount > 0 ? (bandWidth - groupGap * (seriesCount - 1)) / seriesCount : bandWidth

        lines.forEach((line, idx) => {
          const value = d[line.dataKey]
          if (typeof value === 'number') {
            const axisScale = yScales[normalizeYAxisId(line.yAxisId)] ?? primaryYScale
            const baselineY = axisScale(0) ?? innerHeight
            const valueY = axisScale(value) ?? 0
            const barLengthPx = baselineY - valueY

            if (squareSnap && !isHorizontal && value > 0) {
              yPositions[line.dataKey] = topSquareCenterY({
                baselineY,
                barLengthPx,
                squareSize: individualBarWidth,
                gap: squareSnap.squareGap,
                fit: squareSnap.fit,
              })
            } else {
              yPositions[line.dataKey] = valueY
            }

            xPositions[line.dataKey] =
              barPos + idx * (individualBarWidth + groupGap) + individualBarWidth / 2
          }
        })
      }

      // Tooltip position: for horizontal, position at max bar end; for vertical, center of band
      let tooltipX: number
      if (isHorizontal) {
        // Position tooltip at the end of the longest bar
        const maxX = Math.max(...Object.values(xPositions), 0)
        tooltipX = maxX
      } else {
        tooltipX = barPos + bandWidth / 2
      }

      scheduleTooltip({
        point: d,
        index: clampedIndex,
        x: tooltipX,
        yPositions,
        xPositions: Object.keys(xPositions).length > 0 ? xPositions : undefined,
      })
    },
    [
      categoryScale,
      valueScale,
      data,
      lines,
      margin.left,
      margin.top,
      categoryAccessor,
      columnWidth,
      bandWidth,
      isHorizontal,
      stacked,
      stackGap,
      scheduleTooltip,
      yScales,
      primaryYScale,
      squareSnap,
      innerHeight,
    ],
  )

  const handleMouseLeave = useCallback(() => {
    clearTooltip()
  }, [clearTooltip])

  const canInteract = isLoaded

  // Separate children into defs, pre-overlay, and post-overlay
  const defsChildren: ReactElement[] = []
  const clipExcludedChildren: ReactElement[] = []
  const underlayChildren: ReactElement[] = []
  const preOverlayChildren: ReactElement[] = []
  const postOverlayChildren: ReactElement[] = []

  forEachChartChild(children, (child) => {
    const resolvedChild = resolveChartChildElement(child)

    if (isGradientDefComponent(child)) {
      defsChildren.push(child)
    } else if (isPatternDefComponent(child)) {
      preOverlayChildren.push(child)
    } else if (isPostOverlayComponent(resolvedChild)) {
      postOverlayChildren.push(resolvedChild)
    } else if (isClipExcludedComponent(resolvedChild)) {
      clipExcludedChildren.push(isChartClipPassthrough(child.type) ? resolvedChild : child)
    } else if (isUnderlayComponent(resolvedChild)) {
      underlayChildren.push(resolvedChild)
    } else {
      preOverlayChildren.push(child)
    }
  })

  const referenceAreas = useMemo(() => extractReferenceAreaConfigs(children), [children])

  const contextValue = {
    ...DEFAULT_CHART_LIFECYCLE,
    chartPhase: resolveRestingChartPhase(status),
    chartStatus: status,
    data,
    renderData: data,
    xScale: fakeTimeScale as unknown as ReturnType<typeof import('@visx/scale').scaleTime<number>>,
    yScale: isHorizontal ? valueScale : primaryYScale,
    yScales,
    width,
    height,
    innerWidth,
    innerHeight,
    margin,
    columnWidth,
    tooltipData,
    setTooltipData,
    containerRef,
    lines,
    referenceAreas,
    isLoaded,
    animationDuration,
    animationEasing,
    enterTransition,
    revealEpoch,
    xAccessor: xAccessorDate,
    dateLabels,
    // Bar-specific properties
    barScale: categoryScale,
    bandWidth,
    hoveredBarIndex,
    barXAccessor: categoryAccessor,
    orientation,
    stacked,
    stackOffsets,
    squareSnap,
  }

  return (
    <ChartProvider value={contextValue}>
      <svg aria-hidden="true" className="overflow-visible" height={height} width={width}>
        {/* Gradient and pattern definitions */}
        {defsChildren.length > 0 && <defs>{defsChildren}</defs>}

        <rect fill="transparent" height={height} width={width} x={0} y={0} />

        {/* Chart interaction area (pointer-only; the Table views are the keyboard path). */}
        <g
          onMouseLeave={canInteract ? handleMouseLeave : undefined}
          onMouseMove={canInteract ? handleMouseMove : undefined}
          style={{ cursor: canInteract ? 'crosshair' : 'default' }}
          transform={`translate(${margin.left},${margin.top})`}
        >
          {/* Background rect for mouse event detection */}
          <rect fill="transparent" height={innerHeight} width={innerWidth} x={0} y={0} />

          {renderKeyedChartLayers(clipExcludedChildren)}
          {renderKeyedChartLayers(underlayChildren)}
          {status === 'loading' ? (
            <BarLoadingSkeleton
              barCount={data.length || FALLBACK_LOADING_BARS}
              innerHeight={innerHeight}
              innerWidth={innerWidth}
            />
          ) : (
            renderKeyedChartLayers(preOverlayChildren)
          )}

          {/* Markers rendered last so they're on top for interaction */}
          {renderKeyedChartLayers(postOverlayChildren)}
        </g>
      </svg>
    </ChartProvider>
  )
})

export function BarChart({
  data,
  xDataKey = 'name',
  margin: marginProp,
  animationDuration = 1100,
  animationEasing = DEFAULT_ANIMATION_EASING,
  enterTransition,
  revealSignature,
  aspectRatio = '2 / 1',
  className = '',
  barGap = 0.2,
  barWidth,
  orientation = 'vertical',
  stacked = false,
  stackGap = 0,
  squareSnap,
  children,
  onPhaseChange,
  status = 'ready',
}: BarChartProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const margin = { ...DEFAULT_MARGIN, ...marginProp }

  return (
    <div
      className={cn('relative w-full overflow-visible', className)}
      ref={containerRef}
      style={{ aspectRatio }}
    >
      <ParentSize debounceTime={10}>
        {({ width, height }) => (
          <ChartInner
            animationDuration={animationDuration}
            animationEasing={animationEasing}
            barGap={barGap}
            barWidthProp={barWidth}
            containerRef={containerRef}
            data={data}
            enterTransition={enterTransition}
            height={height}
            margin={margin}
            onPhaseChange={onPhaseChange}
            orientation={orientation}
            revealSignature={revealSignature}
            squareSnap={squareSnap}
            stacked={stacked}
            stackGap={stackGap}
            status={status}
            width={width}
            xDataKey={xDataKey}
          >
            {children}
          </ChartInner>
        )}
      </ParentSize>
    </div>
  )
}

BarChart.displayName = 'BarChart'

export default BarChart
