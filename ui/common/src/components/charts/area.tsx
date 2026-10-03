'use client'

// Lab: the real d3 curve type, not the kit's `any`.
import type { CurveFactory } from 'd3-shape'
import { curveLinear } from '@visx/curve'
import { AreaClosed, LinePath } from '@visx/shape'

import { useCallback, useId, useMemo, useRef, useState } from 'react'
import { AreaGradientDefs } from './area-gradient-defs'
import { chartCssVars, useChartStable, useYScale } from './chart-context'
import type { ChartPhase, LoadingStyle } from './chart-phase'
import { type FadeEdges, resolveFadeSides } from './fade-edges'
import { type LineLoadingPulseMode, LineLoadingPulseStroke } from './line-loading-pulse'
import { resolveLineLoadingPulseMode } from './line-loading-timing'
import { LINE_LOADING_LOOP_PAUSE_MS } from './line-loading-timing'
import { LineLoadingSweep } from './loading-sweep'
import { resolveDashTailBounds, usePathStrokeMetrics } from './path-stroke-utils'
import { SeriesDashTailOverlay } from './series-dash-tail-overlay'
import { SeriesHighlightLayer } from './series-highlight-layer'
import { SeriesHoverDim } from './series-hover-dim'
import { SeriesMarkers } from './series-markers'
import type { SeriesPointMarkerStyle } from './series-point-marker'

export interface AreaProps {
  /** Key in data to use for y values */
  dataKey: string
  /** Y-scale group id (Recharts `yAxisId`). Default: `"left"`. */
  yAxisId?: string | number
  /** Fill color for the area gradient start. Default: var(--chart-line-primary) */
  fill?: string
  /** Fill opacity at the top of the area. Default: 0.4 */
  fillOpacity?: number
  /** Stroke color for the line. Default: same as fill */
  stroke?: string
  /** Stroke width. Default: 2 */
  strokeWidth?: number
  /** Curve function. Default: curveLinear (a smoothed curve invents values between points) */
  curve?: CurveFactory
  /** Whether to animate the area. Default: true */
  animate?: boolean
  /** Whether to show the stroke line. Default: true */
  showLine?: boolean
  /** Whether to show highlight segment on hover. Default: true */
  showHighlight?: boolean
  /** Gradient opacity at bottom (0 = fully transparent). Default: 0 */
  gradientToOpacity?: number
  /**
   * Vertical extent of the fill gradient (0–1). `1` fades across the full
   * height; lower values compress the gradient toward the top.
   */
  gradientSpan?: number
  /**
   * Fade the area fill (and stroke) toward transparent at the chart edges.
   * - `true` fades both edges, `false` disables the fade entirely.
   * - `"left"` / `"right"` fades only that side — useful when the opposite
   *   edge butts up against another element you don't want to fade into.
   * Default: false
   */
  fadeEdges?: FadeEdges
  /** Render scatter-style circle markers at each data point. Default: false */
  showMarkers?: boolean
  /** Marker styling (same options as Scatter). */
  markers?: SeriesPointMarkerStyle
  /**
   * Data index from which the line stroke becomes dashed (inclusive).
   * Useful for projecting incomplete periods, e.g. dashed from yesterday through today.
   */
  dashFromIndex?: number
  /** Dash pattern for the tail segment when `dashFromIndex` is set. Default: "6,4" */
  dashArray?: string
  /** Pulse stroke color while chart is loading. Default: var(--foreground) */
  loadingStroke?: string
  /** Pulse stroke opacity while chart is loading. Default: 0.5 */
  loadingStrokeOpacity?: number
  /**
   * Show the loading pulse overlay. Default: follows chart loading phase.
   * Set `false` to disable even during loading.
   */
  loading?: boolean
  /** Override pulse animation mode (loop / exit / enter). */
  loadingPulseMode?: LineLoadingPulseMode
  /**
   * Loading animation while the chart is in loading status: the default
   * traveling `"pulse"`, or a diagonal `"sweep"` shimmer across the skeleton
   * area. Default: `"pulse"`.
   */
  loadingStyle?: LoadingStyle
}

function useAreaLoadingPulseState(
  chartPhase: ChartPhase,
  loading: boolean | undefined,
  loadingPulseMode: LineLoadingPulseMode | undefined,
  notifyLoadingPulseComplete?: () => void,
) {
  const phasePulseMode = resolveLineLoadingPulseMode(chartPhase)
  const pulseMode =
    loading === false ? null : (loadingPulseMode ?? (loading === true ? 'loop' : phasePulseMode))
  const showLoadingPulse = pulseMode != null
  const showSeriesContent =
    chartPhase === 'revealing' || chartPhase === 'ready' || chartPhase === 'exitingReady'
  const [pulseEpoch, setPulseEpoch] = useState(0)

  const handleLoadingPulseComplete = useCallback(() => {
    if (pulseMode === 'loop') {
      window.setTimeout(() => {
        setPulseEpoch((epoch) => epoch + 1)
      }, LINE_LOADING_LOOP_PAUSE_MS)
      return
    }
    notifyLoadingPulseComplete?.()
  }, [notifyLoadingPulseComplete, pulseMode])

  return {
    handleLoadingPulseComplete,
    pulseMode,
    pulseEpoch,
    showLoadingPulse,
    showSeriesContent,
  }
}

// Note: mirrors Line series layout (fill, stroke, dash, markers, pulse)
export function Area({
  dataKey,
  yAxisId,
  fill = chartCssVars.linePrimary,
  fillOpacity = 0.4,
  stroke,
  strokeWidth = 2,
  curve = curveLinear,
  animate = true,
  showLine = true,
  showHighlight = true,
  gradientToOpacity = 0,
  gradientSpan = 1,
  fadeEdges = false,
  showMarkers = false,
  markers,
  dashFromIndex,
  dashArray = '6,4',
  loading,
  loadingStroke = chartCssVars.foreground,
  loadingStrokeOpacity = 0.5,
  loadingPulseMode,
  loadingStyle = 'pulse',
}: AreaProps) {
  // Stable slice only: hover state lives inside `<SeriesHoverDim>` and
  // `<SeriesHighlightLayer>` so this component (and its expensive
  // <SeriesDashTailOverlay> child) does not re-render on cursor motion.
  // The reveal-clip is now a single shared clipPath at the chart-shell
  // level (`time-series-chart-shell.tsx`); we no longer render a per-area
  // `<ChartRevealClip>` or read `revealEpoch` here.
  const {
    data,
    renderData,
    xScale,
    innerHeight,
    innerWidth,
    xAccessor,
    lines,
    chartPhase,
    notifyLoadingPulseComplete,
  } = useChartStable()
  const yScale = useYScale(yAxisId)
  const { handleLoadingPulseComplete, pulseMode, pulseEpoch, showLoadingPulse, showSeriesContent } =
    useAreaLoadingPulseState(chartPhase, loading, loadingPulseMode, notifyLoadingPulseComplete)

  const seriesIndex = useMemo(() => {
    const index = lines.findIndex((line) => line.dataKey === dataKey)
    return index >= 0 ? index : 0
  }, [lines, dataKey])

  const pathRef = useRef<SVGPathElement>(null)
  const { pathLength, pathD } = usePathStrokeMetrics(pathRef, [
    renderData,
    innerWidth,
    dashFromIndex,
    showLine,
    showSeriesContent,
    showLoadingPulse,
  ])

  // Unique IDs for this area
  const uniqueId = useId()
  const gradientId = `area-gradient-${dataKey}-${uniqueId}`
  const strokeGradientId = `area-stroke-gradient-${dataKey}-${uniqueId}`
  const edgeMaskId = `area-edge-mask-${dataKey}-${uniqueId}`
  const edgeGradientId = `${edgeMaskId}-gradient`

  const isPatternFill = fill.startsWith('url(')
  const showAreaFill = isPatternFill || fillOpacity > 0
  const areaFill = isPatternFill ? fill : `url(#${gradientId})`

  // Resolved stroke color (defaults to fill; pattern URLs need a real color)
  const resolvedStroke = stroke || (isPatternFill ? chartCssVars.linePrimary : fill)

  const getY = useCallback(
    (d: Record<string, unknown>) => {
      const value = d[dataKey]
      return typeof value === 'number' ? (yScale(value) ?? 0) : 0
    },
    [dataKey, yScale],
  )

  const hasDashTail = resolveDashTailBounds(dashFromIndex, data.length)
  // The stroke gradient is only emitted when at least one edge fades, so fall
  // back to the resolved solid color otherwise — avoids an invalid url(#...).
  const fadeSides = resolveFadeSides(fadeEdges)
  const useViewportEdgeFade = fadeSides.any && !isPatternFill
  let strokePaint = resolvedStroke
  if (!useViewportEdgeFade && fadeSides.any) {
    strokePaint = `url(#${strokeGradientId})`
  }
  const highlightEnabled = showHighlight && showLine && !showLoadingPulse && showSeriesContent
  const showSeriesStroke = showSeriesContent && showLine
  let visibleStroke = 'transparent'
  if (showSeriesStroke && !hasDashTail) {
    visibleStroke = strokePaint
  }
  const shouldMeasurePath = showLine && (showSeriesContent || showLoadingPulse)

  const seriesLayers = (
    <>
      {showSeriesContent && showAreaFill ? (
        <AreaClosed
          curve={curve}
          data={renderData}
          fill={areaFill}
          x={(d) => xScale(xAccessor(d)) ?? 0}
          y={getY}
          yScale={yScale}
        />
      ) : null}

      {shouldMeasurePath ? (
        <>
          <LinePath
            curve={curve}
            data={renderData}
            innerRef={pathRef}
            stroke={visibleStroke}
            strokeLinecap="round"
            strokeWidth={strokeWidth}
            x={(d) => xScale(xAccessor(d)) ?? 0}
            y={getY}
          />
          {showSeriesStroke ? (
            <SeriesDashTailOverlay
              dashArray={dashArray}
              dashFromIndex={dashFromIndex}
              data={data}
              innerHeight={innerHeight}
              innerWidth={innerWidth}
              pathD={pathD}
              pathLength={pathLength}
              stroke={strokePaint}
              strokeWidth={strokeWidth}
              xAccessor={xAccessor}
              xScale={xScale}
            />
          ) : null}
        </>
      ) : null}
    </>
  )

  // Sweep style owns all loading modes (loop + the exit/enter transitions),
  // drawing its own silhouette; the pulse covers the default style.
  const sweepLoading = showLoadingPulse && innerWidth > 0 && loadingStyle === 'sweep'
  const pulseLoading = showLoadingPulse && innerWidth > 0 && !sweepLoading

  return (
    <>
      <AreaGradientDefs
        edgeGradientId={edgeGradientId}
        edgeMaskId={edgeMaskId}
        fadeEdges={fadeEdges}
        fill={fill}
        fillOpacity={fillOpacity}
        gradientId={gradientId}
        gradientSpan={gradientSpan}
        gradientToOpacity={gradientToOpacity}
        innerHeight={innerHeight}
        innerWidth={innerWidth}
        isPatternFill={isPatternFill}
        resolvedStroke={resolvedStroke}
        strokeGradientId={strokeGradientId}
      />

      <SeriesHoverDim dimOpacity={0.6} enabled={showHighlight} seriesIndex={seriesIndex}>
        {useViewportEdgeFade ? <g mask={`url(#${edgeMaskId})`}>{seriesLayers}</g> : seriesLayers}
      </SeriesHoverDim>

      {/* Highlight segment on hover — isolated hover subscriber. */}
      <SeriesHighlightLayer
        enabled={highlightEnabled}
        height={innerHeight}
        pathRef={pathRef}
        stroke={resolvedStroke}
        strokeWidth={strokeWidth}
      />

      {showMarkers && showSeriesContent ? (
        <SeriesMarkers
          animate={animate}
          dataKey={dataKey}
          {...markers}
          fill={markers?.fill ?? resolvedStroke}
          stroke={markers?.stroke ?? markers?.fill ?? resolvedStroke}
        />
      ) : null}

      {sweepLoading ? (
        <LineLoadingSweep
          curve={curve}
          key="loading-sweep"
          mode={pulseMode ?? 'loop'}
          onTransitionComplete={handleLoadingPulseComplete}
          stroke={loadingStroke}
          strokeOpacity={loadingStrokeOpacity}
          strokeWidth={strokeWidth}
          withArea
        />
      ) : null}
      {pulseLoading && pathD ? (
        <LineLoadingPulseStroke
          key="loading-pulse"
          loopEpoch={pulseEpoch}
          mode={pulseMode ?? undefined}
          onCycleComplete={handleLoadingPulseComplete}
          pathD={pathD}
          stroke={loadingStroke}
          strokeOpacity={loadingStrokeOpacity}
          strokeWidth={strokeWidth}
        />
      ) : null}
    </>
  )
}

Area.displayName = 'Area'

export default Area
