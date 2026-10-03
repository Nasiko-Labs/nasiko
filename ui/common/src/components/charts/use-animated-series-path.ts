/* eslint-disable react-hooks/set-state-in-effect -- Kit animation code (from the React migration's visx kit): it mounts portals after the
   first render and drives motion springs from values tracked across renders. Correct without the React Compiler,
   so the file opts out of it with 'use no memo'. */
'use client'
'use no memo'

// Lab: the real d3 curve type, not the kit's `any`.
import type { CurveFactory } from 'd3-shape'
import { animate, useReducedMotion } from 'motion/react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { LINE_LOADING_PULSE_EASE } from './line-loading-timing'
import {
  computeSeriesPathPoints,
  interpolateSeriesPathPoints,
  type SeriesPathPoint,
  seriesPathFromPoints,
  seriesPathTransitionSignature,
} from './series-path-utils'

export interface UseAnimatedSeriesPathOptions {
  renderData: Record<string, unknown>[]
  xAccessor: (datum: Record<string, unknown>) => Date
  xScale: (value: Date) => number | undefined
  yScale: (value: number) => number | undefined
  dataKey: string
  curve: CurveFactory
  chartPhase: string
  durationMs: number
  innerWidth: number
  enabled: boolean
}

export function useAnimatedSeriesPath({
  renderData,
  xAccessor,
  xScale,
  yScale,
  dataKey,
  curve,
  chartPhase,
  durationMs,
  innerWidth,
  enabled,
}: UseAnimatedSeriesPathOptions) {
  const reducedMotion = useReducedMotion()
  const [animatedPoints, setAnimatedPoints] = useState<SeriesPathPoint[] | null>(null)
  const displayedPointsRef = useRef<SeriesPathPoint[] | null>(null)
  const animatingRef = useRef(false)

  const xScaleDomain = useMemo(() => {
    const scaleWithDomain = xScale as { domain?: () => [Date, Date] }
    return scaleWithDomain.domain?.() ?? [new Date(0), new Date(0)]
  }, [xScale])

  const transitionSignature = useMemo(
    () =>
      seriesPathTransitionSignature({
        renderData,
        xAccessor,
        dataKey,
        innerWidth,
        xDomainMin: xScaleDomain[0]?.getTime?.() ?? 0,
        xDomainMax: xScaleDomain[1]?.getTime?.() ?? 0,
      }),
    [renderData, xAccessor, dataKey, innerWidth, xScaleDomain],
  )

  const targetPoints = useMemo(
    () => computeSeriesPathPoints(renderData, xAccessor, xScale, yScale, dataKey),
    [renderData, xAccessor, xScale, yScale, dataKey],
  )

  const prevTransitionSignatureRef = useRef(transitionSignature)

  useEffect(() => {
    if (!animatingRef.current) {
      displayedPointsRef.current = targetPoints
    }
  }, [targetPoints])

  useEffect(() => {
    const shouldAnimate =
      enabled && !reducedMotion && chartPhase === 'ready' && durationMs > 0 && renderData.length > 0

    if (!shouldAnimate) {
      animatingRef.current = false
      setAnimatedPoints(null)
      displayedPointsRef.current = targetPoints
      prevTransitionSignatureRef.current = transitionSignature
      return
    }

    if (prevTransitionSignatureRef.current === transitionSignature) {
      return
    }
    prevTransitionSignatureRef.current = transitionSignature

    const fromPoints = displayedPointsRef.current ?? targetPoints
    if (fromPoints.length === 0) {
      displayedPointsRef.current = targetPoints
      return
    }

    animatingRef.current = true
    const fromSnapshot = fromPoints

    const control = animate(0, 1, {
      duration: durationMs / 1000,
      ease: [...LINE_LOADING_PULSE_EASE],
      onUpdate: (progress) => {
        const currentTarget = computeSeriesPathPoints(
          renderData,
          xAccessor,
          xScale,
          yScale,
          dataKey,
        )
        const next = interpolateSeriesPathPoints(fromSnapshot, currentTarget, progress)
        displayedPointsRef.current = next
        setAnimatedPoints(next)
      },
      onComplete: () => {
        animatingRef.current = false
        displayedPointsRef.current = targetPoints
        setAnimatedPoints(null)
      },
    })

    return () => {
      control.stop()
      animatingRef.current = false
    }
  }, [
    transitionSignature,
    chartPhase,
    durationMs,
    enabled,
    reducedMotion,
    renderData,
    xAccessor,
    xScale,
    yScale,
    dataKey,
    targetPoints,
  ])

  const activePoints = animatedPoints ?? targetPoints
  const pathD = useMemo(() => seriesPathFromPoints(activePoints, curve), [activePoints, curve])

  return {
    pathD,
    isPathAnimating: animatedPoints != null,
  }
}
