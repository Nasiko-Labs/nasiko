/* eslint-disable react-hooks/refs -- Kit animation code (from the React migration's visx kit): it mounts portals after the
   first render and drives motion springs from values tracked across renders. Correct without the React Compiler,
   so the file opts out of it with 'use no memo'. */
'use client'
'use no memo'

import { animate, useReducedMotion } from 'motion/react'
import { useEffect, useRef, useState } from 'react'
import type { ChartPhase } from './chart-phase'
import { LINE_LOADING_PULSE_EASE } from './line-loading-timing'
import {
  domainsEqual,
  isYDomainTweenPhase,
  resolveAnimatedYDestinationDomains,
  shouldTweenYDomain,
  type YDomain,
} from './y-domain-utils'

function lerpDomain(from: YDomain, to: YDomain, progress: number): YDomain {
  return [from[0] + (to[0] - from[0]) * progress, from[1] + (to[1] - from[1]) * progress]
}

function snapDomains(
  domains: Record<string, YDomain>,
  setAnimatedByAxis: (domains: Record<string, YDomain>) => void,
  animatedRef: { current: Record<string, YDomain> },
) {
  if (domainsEqual(animatedRef.current, domains)) {
    return
  }
  setAnimatedByAxis(domains)
  animatedRef.current = domains
}

function tweenDomains({
  destination,
  durationMs,
  enabled,
  reducedMotion,
  animatedRef,
  setAnimatedByAxis,
  onSettled,
}: {
  destination: Record<string, YDomain>
  durationMs: number
  enabled: boolean
  reducedMotion: boolean | null
  animatedRef: { current: Record<string, YDomain> }
  setAnimatedByAxis: (domains: Record<string, YDomain>) => void
  onSettled?: () => void
}) {
  if (domainsEqual(animatedRef.current, destination)) {
    onSettled?.()
    return
  }

  if (!enabled || reducedMotion) {
    snapDomains(destination, setAnimatedByAxis, animatedRef)
    onSettled?.()
    return
  }

  const axisIds = Object.keys(destination)
  const fromSnapshot = animatedRef.current

  let needsTween = false
  for (const axisId of axisIds) {
    const from = fromSnapshot[axisId] ?? destination[axisId] ?? ([0, 100] as YDomain)
    const to = destination[axisId] ?? from
    if (shouldTweenYDomain(from, to)) {
      needsTween = true
      break
    }
  }

  if (!needsTween) {
    snapDomains(destination, setAnimatedByAxis, animatedRef)
    onSettled?.()
    return
  }

  const fromByAxis: Record<string, YDomain> = {}
  for (const axisId of axisIds) {
    fromByAxis[axisId] = fromSnapshot[axisId] ?? destination[axisId] ?? [0, 100]
  }

  const control = animate(0, 1, {
    duration: durationMs / 1000,
    ease: [...LINE_LOADING_PULSE_EASE],
    onUpdate: (progress) => {
      const next: Record<string, YDomain> = {}
      for (const axisId of axisIds) {
        const from = fromByAxis[axisId] ?? destination[axisId] ?? ([0, 100] as YDomain)
        const to = destination[axisId] ?? from
        next[axisId] = shouldTweenYDomain(from, to) ? lerpDomain(from, to, progress) : to
      }
      animatedRef.current = next
      setAnimatedByAxis(next)
    },
    onComplete: () => {
      snapDomains(destination, setAnimatedByAxis, animatedRef)
      onSettled?.()
    },
  })

  return control
}

export interface UseAnimatedYDomainsOptions {
  enabled: boolean
  durationMs: number
  chartPhase: ChartPhase
  skeletonByAxis: Record<string, YDomain>
  targetByAxis: Record<string, YDomain>
  onSettled?: () => void
  /** When true, tweens y-domains on target changes while the chart is in the ready phase (e.g. brush zoom). */
  tweenOnTargetChange?: boolean
}

export function useAnimatedYDomains({
  enabled,
  durationMs,
  chartPhase,
  skeletonByAxis,
  targetByAxis,
  onSettled,
  tweenOnTargetChange = false,
}: UseAnimatedYDomainsOptions): Record<string, YDomain> {
  const reducedMotion = useReducedMotion()
  const destinationByAxis = resolveAnimatedYDestinationDomains(
    chartPhase,
    skeletonByAxis,
    targetByAxis,
  )
  const destinationRef = useRef(destinationByAxis)
  destinationRef.current = destinationByAxis
  const skeletonRef = useRef(skeletonByAxis)
  skeletonRef.current = skeletonByAxis
  const targetRef = useRef(targetByAxis)
  targetRef.current = targetByAxis

  const [animatedByAxis, setAnimatedByAxis] = useState(destinationByAxis)
  const animatedRef = useRef(animatedByAxis)
  const prevPhaseRef = useRef(chartPhase)
  const onSettledRef = useRef(onSettled)
  onSettledRef.current = onSettled

  useEffect(() => {
    animatedRef.current = animatedByAxis
  }, [animatedByAxis])

  useEffect(() => {
    if (prevPhaseRef.current === chartPhase) {
      return
    }
    prevPhaseRef.current = chartPhase

    const settle = () => {
      onSettledRef.current?.()
    }

    // Keep grid spacing frozen while the series exits the viewport.
    if (chartPhase === 'exiting') {
      snapDomains(skeletonRef.current, setAnimatedByAxis, animatedRef)
      return
    }
    if (chartPhase === 'exitingReady') {
      snapDomains(targetRef.current, setAnimatedByAxis, animatedRef)
      return
    }
    if (chartPhase === 'loading') {
      snapDomains(skeletonRef.current, setAnimatedByAxis, animatedRef)
      return
    }
    if (chartPhase === 'revealing' || chartPhase === 'ready') {
      snapDomains(targetRef.current, setAnimatedByAxis, animatedRef)
      return
    }

    if (!isYDomainTweenPhase(chartPhase)) {
      return
    }

    const control = tweenDomains({
      destination: destinationRef.current,
      durationMs,
      enabled,
      reducedMotion,
      animatedRef,
      setAnimatedByAxis,
      onSettled: settle,
    })

    return () => control?.stop()
  }, [chartPhase, durationMs, enabled, reducedMotion])

  const targetSignature = JSON.stringify(targetByAxis)
  const prevTargetSignatureRef = useRef(targetSignature)

  useEffect(() => {
    const inLivePhase = chartPhase === 'ready' || chartPhase === 'revealing'

    if (!inLivePhase) {
      prevTargetSignatureRef.current = targetSignature
      return
    }

    if (prevTargetSignatureRef.current === targetSignature) {
      return
    }
    prevTargetSignatureRef.current = targetSignature

    if (tweenOnTargetChange && chartPhase === 'ready') {
      const control = tweenDomains({
        destination: targetRef.current,
        durationMs,
        enabled,
        reducedMotion,
        animatedRef,
        setAnimatedByAxis,
        onSettled: () => onSettledRef.current?.(),
      })

      return () => control?.stop()
    }

    snapDomains(targetRef.current, setAnimatedByAxis, animatedRef)
  }, [chartPhase, durationMs, enabled, reducedMotion, targetSignature, tweenOnTargetChange])

  return animatedByAxis
}
