'use client'

import { localPoint } from '@visx/event'
import type { scaleLinear, scaleTime } from '@visx/scale'
import { useCallback, useEffect, useRef, useState } from 'react'
import type { LineConfig, Margin, TooltipData } from './chart-context'
import { useScheduledTooltip } from './use-scheduled-tooltip'
import { normalizeYAxisId } from './y-axis-scales'

type ScaleTime = ReturnType<typeof scaleTime<number>>
type ScaleLinear = ReturnType<typeof scaleLinear<number>>

export interface ChartSelection {
  startX: number
  endX: number
  startIndex: number
  endIndex: number
  active: boolean
}

interface UseChartInteractionParams {
  xScale: ScaleTime
  yScale: ScaleLinear
  yScales: Record<string, ScaleLinear>
  data: Record<string, unknown>[]
  lines: LineConfig[]
  margin: Margin
  xAccessor: (d: Record<string, unknown>) => Date
  bisectDate: (data: Record<string, unknown>[], date: Date, lo: number) => number
  canInteract: boolean
}

interface ChartInteractionResult {
  tooltipData: TooltipData | null
  setTooltipData: React.Dispatch<React.SetStateAction<TooltipData | null>>
  selection: ChartSelection | null
  clearSelection: () => void
  interactionHandlers: {
    onMouseMove?: (event: React.MouseEvent<SVGGElement>) => void
    onMouseLeave?: () => void
    onMouseDown?: (event: React.MouseEvent<SVGGElement>) => void
    onMouseUp?: () => void
    onTouchStart?: (event: React.TouchEvent<SVGGElement>) => void
    onTouchMove?: (event: React.TouchEvent<SVGGElement>) => void
    onTouchEnd?: () => void
  }
  interactionStyle: React.CSSProperties
}

export function useChartInteraction({
  xScale,
  yScale,
  yScales,
  data,
  lines,
  margin,
  xAccessor,
  bisectDate,
  canInteract,
}: UseChartInteractionParams): ChartInteractionResult {
  const [selection, setSelection] = useState<ChartSelection | null>(null)
  const { tooltipData, setTooltipData, scheduleTooltip, clearTooltip, resetTooltipDedupe } =
    useScheduledTooltip<TooltipData>()

  const isDraggingRef = useRef(false)
  const dragStartXRef = useRef<number>(0)
  const lastHoveredXRef = useRef<number | null>(null)

  const resolveTooltipFromX = useCallback(
    (pixelX: number): TooltipData | null => {
      const x0 = xScale.invert(pixelX)
      const index = bisectDate(data, x0, 1)
      const d0 = data[index - 1]
      const d1 = data[index]

      if (!d0) {
        return null
      }

      let d = d0
      let finalIndex = index - 1
      if (d1) {
        const d0Time = xAccessor(d0).getTime()
        const d1Time = xAccessor(d1).getTime()
        if (x0.getTime() - d0Time > d1Time - x0.getTime()) {
          d = d1
          finalIndex = index
        }
      }

      const yPositions: Record<string, number> = {}
      for (const line of lines) {
        const value = d[line.dataKey]
        if (typeof value === 'number') {
          const axisScale = yScales[normalizeYAxisId(line.yAxisId)] ?? yScale
          yPositions[line.dataKey] = axisScale(value) ?? 0
        }
      }

      return {
        point: d,
        index: finalIndex,
        x: xScale(xAccessor(d)) ?? 0,
        yPositions,
      }
    },
    [xScale, yScale, yScales, data, lines, xAccessor, bisectDate],
  )

  const resolveIndexFromX = useCallback(
    (pixelX: number): number => {
      const x0 = xScale.invert(pixelX)
      const index = bisectDate(data, x0, 1)
      const d0 = data[index - 1]
      const d1 = data[index]
      if (!d0) {
        return 0
      }
      if (d1) {
        const d0Time = xAccessor(d0).getTime()
        const d1Time = xAccessor(d1).getTime()
        if (x0.getTime() - d0Time > d1Time - x0.getTime()) {
          return index
        }
      }
      return index - 1
    },
    [xScale, data, xAccessor, bisectDate],
  )

  const getChartX = useCallback(
    (
      event: React.MouseEvent<SVGGElement> | React.TouchEvent<SVGGElement>,
      touchIndex = 0,
    ): number | null => {
      let point: { x: number; y: number } | null = null

      if ('touches' in event) {
        const touch = event.touches[touchIndex]
        if (!touch) {
          return null
        }
        const svg = event.currentTarget.ownerSVGElement
        if (!svg) {
          return null
        }
        point = localPoint(svg, touch as unknown as MouseEvent)
      } else {
        point = localPoint(event)
      }

      if (!point) {
        return null
      }
      return point.x - margin.left
    },
    [margin.left],
  )

  const handleMouseMove = useCallback(
    (event: React.MouseEvent<SVGGElement>) => {
      const chartX = getChartX(event)
      if (chartX === null) {
        return
      }

      if (isDraggingRef.current) {
        const startX = Math.min(dragStartXRef.current, chartX)
        const endX = Math.max(dragStartXRef.current, chartX)
        setSelection({
          startX,
          endX,
          startIndex: resolveIndexFromX(startX),
          endIndex: resolveIndexFromX(endX),
          active: true,
        })
        return
      }

      lastHoveredXRef.current = chartX
      const tooltip = resolveTooltipFromX(chartX)
      if (tooltip) {
        scheduleTooltip(tooltip)
      }
    },
    [getChartX, resolveTooltipFromX, resolveIndexFromX, scheduleTooltip],
  )

  const handleMouseLeave = useCallback(() => {
    lastHoveredXRef.current = null
    clearTooltip()
    if (isDraggingRef.current) {
      isDraggingRef.current = false
    }
    setSelection(null)
  }, [clearTooltip])

  const handleMouseDown = useCallback(
    (event: React.MouseEvent<SVGGElement>) => {
      const chartX = getChartX(event)
      if (chartX === null) {
        return
      }
      isDraggingRef.current = true
      dragStartXRef.current = chartX
      clearTooltip()
      setSelection(null)
    },
    [getChartX, clearTooltip],
  )

  const handleMouseUp = useCallback(() => {
    if (isDraggingRef.current) {
      isDraggingRef.current = false
    }
    setSelection(null)
  }, [])

  const handleTouchStart = useCallback(
    (event: React.TouchEvent<SVGGElement>) => {
      if (event.touches.length === 1) {
        event.preventDefault()
        const chartX = getChartX(event, 0)
        if (chartX === null) {
          return
        }
        lastHoveredXRef.current = chartX
        const tooltip = resolveTooltipFromX(chartX)
        if (tooltip) {
          scheduleTooltip(tooltip)
        }
      } else if (event.touches.length === 2) {
        event.preventDefault()
        resetTooltipDedupe()
        clearTooltip()
        const x0 = getChartX(event, 0)
        const x1 = getChartX(event, 1)
        if (x0 === null || x1 === null) {
          return
        }
        const startX = Math.min(x0, x1)
        const endX = Math.max(x0, x1)
        setSelection({
          startX,
          endX,
          startIndex: resolveIndexFromX(startX),
          endIndex: resolveIndexFromX(endX),
          active: true,
        })
      }
    },
    [
      getChartX,
      resolveTooltipFromX,
      resolveIndexFromX,
      scheduleTooltip,
      resetTooltipDedupe,
      clearTooltip,
    ],
  )

  const handleTouchMove = useCallback(
    (event: React.TouchEvent<SVGGElement>) => {
      if (event.touches.length === 1) {
        event.preventDefault()
        const chartX = getChartX(event, 0)
        if (chartX === null) {
          return
        }
        lastHoveredXRef.current = chartX
        const tooltip = resolveTooltipFromX(chartX)
        if (tooltip) {
          scheduleTooltip(tooltip)
        }
      } else if (event.touches.length === 2) {
        event.preventDefault()
        const x0 = getChartX(event, 0)
        const x1 = getChartX(event, 1)
        if (x0 === null || x1 === null) {
          return
        }
        const startX = Math.min(x0, x1)
        const endX = Math.max(x0, x1)
        setSelection({
          startX,
          endX,
          startIndex: resolveIndexFromX(startX),
          endIndex: resolveIndexFromX(endX),
          active: true,
        })
      }
    },
    [getChartX, resolveTooltipFromX, resolveIndexFromX, scheduleTooltip],
  )

  const handleTouchEnd = useCallback(() => {
    clearTooltip()
    setSelection(null)
  }, [clearTooltip])

  const clearSelection = useCallback(() => {
    setSelection(null)
  }, [])

  // Re-anchor tooltip/crosshair when x-scale or visible data changes (e.g. brush zoom commit).
  useEffect(() => {
    if (!canInteract || lastHoveredXRef.current === null) {
      return
    }
    const tooltip = resolveTooltipFromX(lastHoveredXRef.current)
    if (tooltip) {
      scheduleTooltip(tooltip, `${tooltip.index}:${Math.round(tooltip.x)}`)
      return
    }
    clearTooltip()
  }, [canInteract, clearTooltip, resolveTooltipFromX, scheduleTooltip])

  const interactionHandlers = canInteract
    ? {
        onMouseMove: handleMouseMove,
        onMouseLeave: handleMouseLeave,
        onMouseDown: handleMouseDown,
        onMouseUp: handleMouseUp,
        onTouchStart: handleTouchStart,
        onTouchMove: handleTouchMove,
        onTouchEnd: handleTouchEnd,
      }
    : {}

  const interactionStyle: React.CSSProperties = {
    cursor: canInteract ? 'crosshair' : 'default',
    touchAction: 'none',
  }

  return {
    tooltipData,
    setTooltipData,
    selection,
    clearSelection,
    interactionHandlers,
    interactionStyle,
  }
}
