/* eslint-disable react-hooks/refs, react-hooks/set-state-in-effect -- Kit animation code (from the React migration's visx kit): it mounts portals after the
   first render and drives motion springs from values tracked across renders. Correct without the React Compiler,
   so the file opts out of it with 'use no memo'. */
'use client'
'use no memo'

import { motion, useSpring } from 'motion/react'
import type { RefObject } from 'react'
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { cn } from '@/lib/utils'
import { type SpringConfig, useChartConfig } from '../chart-config-context'
import { chartCssVars } from '../chart-context'

export interface TooltipBoxProps {
  /** X position in pixels (relative to container) */
  x: number
  /** Y position in pixels (relative to container) */
  y: number
  /** Whether the tooltip is visible */
  visible: boolean
  /** Container ref for portal rendering */
  containerRef: RefObject<HTMLDivElement | null>
  /** Container width for flip detection */
  containerWidth: number
  /** Container height for bounds clamping */
  containerHeight: number
  /** Offset from the target position */
  offset?: number
  /** Custom class name */
  className?: string
  /** Tooltip content */
  children: React.ReactNode
  /** Override left position (bypasses internal calculation) */
  left?: number | ReturnType<typeof useSpring>
  /** Override top position (bypasses internal calculation) */
  top?: number | ReturnType<typeof useSpring>
  /** Force flip direction (for custom positioning) */
  flipped?: boolean
  /** Per-chart override; falls back to `ChartConfigProvider.tooltipBoxSpring`. */
  springConfig?: SpringConfig
  /** Animate panel position with a spring. Default: true */
  animate?: boolean
  /** Fade/scale the panel on show. Default: true */
  entrance?: boolean
  /** Inline styles for the inner tooltip panel. */
  panelStyle?: React.CSSProperties
  /**
   * Tooltip panel background color (CSS variable or color value).
   * Default: `var(--chart-tooltip-background)`.
   */
  backgroundColor?: string
}

// Inner-only-on-visible so `useSpring` initializes at the cursor's actual x/y
// instead of (0, 0) on first hover.
export function TooltipBox(props: TooltipBoxProps) {
  const [mounted, setMounted] = useState(false)

  useEffect(() => {
    setMounted(true)
  }, [])

  const container = props.containerRef.current
  if (!(mounted && container)) {
    return null
  }
  if (!props.visible) {
    return null
  }
  return <TooltipBoxInner {...props} container={container} />
}

function TooltipBoxInner({
  x,
  y,
  containerWidth,
  containerHeight,
  offset = 16,
  className = '',
  children,
  left: leftOverride,
  top: topOverride,
  flipped: flippedOverride,
  springConfig,
  animate = true,
  entrance = true,
  panelStyle,
  backgroundColor = chartCssVars.tooltipBackground,
  container,
}: Omit<TooltipBoxProps, 'visible' | 'containerRef'> & {
  container: HTMLElement
}) {
  const { tooltipBoxSpring } = useChartConfig()
  const effectiveSpring = springConfig ?? tooltipBoxSpring

  const tooltipRef = useRef<HTMLDivElement>(null)
  const tooltipWidthRef = useRef(180)
  const tooltipHeightRef = useRef(80)
  const [staticPosition, setStaticPosition] = useState({ left: x, top: y })

  const tw = tooltipWidthRef.current
  const th = tooltipHeightRef.current
  const shouldFlipX = x + tw + offset > containerWidth
  const targetX = shouldFlipX ? x - offset - tw : x + offset
  const targetY = Math.max(offset, Math.min(y - th / 2, containerHeight - th - offset))

  const animatedLeft = useSpring(targetX, effectiveSpring)
  const animatedTop = useSpring(targetY, effectiveSpring)

  if (animate && leftOverride === undefined) {
    animatedLeft.set(targetX)
  }
  if (animate && topOverride === undefined) {
    animatedTop.set(targetY)
  }

  useLayoutEffect(() => {
    if (!tooltipRef.current) {
      return
    }
    const el = tooltipRef.current
    const w = el.offsetWidth
    const h = el.offsetHeight
    if (w > 0) {
      tooltipWidthRef.current = w
    }
    if (h > 0) {
      tooltipHeightRef.current = h
    }
    const w2 = tooltipWidthRef.current
    const h2 = tooltipHeightRef.current
    const flip = x + w2 + offset > containerWidth
    const tx = flip ? x - offset - w2 : x + offset
    const ty = Math.max(offset, Math.min(y - h2 / 2, containerHeight - h2 - offset))
    if (!animate) {
      setStaticPosition({ left: tx, top: ty })
      return
    }
    if (leftOverride === undefined) {
      animatedLeft.set(tx)
    }
    if (topOverride === undefined) {
      animatedTop.set(ty)
    }
  }, [
    x,
    y,
    containerWidth,
    containerHeight,
    offset,
    leftOverride,
    topOverride,
    animate,
    animatedLeft,
    animatedTop,
  ])

  const prevFlipRef = useRef(shouldFlipX)
  const [flipKey, setFlipKey] = useState(0)

  useEffect(() => {
    if (prevFlipRef.current !== shouldFlipX) {
      setFlipKey((k) => k + 1)
      prevFlipRef.current = shouldFlipX
    }
  }, [shouldFlipX])

  const finalLeft = animate ? (leftOverride ?? animatedLeft) : staticPosition.left
  const finalTop = animate ? (topOverride ?? animatedTop) : staticPosition.top
  const isFlipped = flippedOverride ?? shouldFlipX
  const transformOrigin = isFlipped ? 'right top' : 'left top'

  const panelClassName = cn(
    'text-chart-tooltip-foreground min-w-[140px] overflow-hidden rounded-lg shadow-lg ring-1 ring-black/10 dark:ring-white/10',
    panelStyle?.backgroundColor === undefined &&
      backgroundColor === chartCssVars.tooltipBackground &&
      'bg-chart-tooltip-background',
    panelStyle?.backdropFilter === undefined && 'backdrop-blur-md',
  )
  const panelStyleResolved = {
    transformOrigin,
    ...(panelStyle?.backgroundColor === undefined && {
      backgroundColor,
    }),
    ...panelStyle,
  }

  if (!entrance) {
    return createPortal(
      <div
        className={cn('pointer-events-none absolute z-50', className)}
        ref={tooltipRef}
        style={{ left: staticPosition.left, top: staticPosition.top }}
      >
        <div className={panelClassName} style={panelStyleResolved}>
          {children}
        </div>
      </div>,
      container,
    )
  }

  return createPortal(
    <motion.div
      animate={{ opacity: 1 }}
      className={cn('pointer-events-none absolute z-50', className)}
      exit={{ opacity: 0 }}
      initial={{ opacity: 0 }}
      ref={tooltipRef}
      style={{ left: finalLeft, top: finalTop }}
      transition={{ duration: 0.1 }}
    >
      <motion.div
        animate={{ scale: 1, opacity: 1, x: 0 }}
        className={panelClassName}
        initial={{ scale: 0.85, opacity: 0, x: isFlipped ? 20 : -20 }}
        key={flipKey}
        style={panelStyleResolved}
        transition={{ type: 'spring', stiffness: 300, damping: 25 }}
      >
        {children}
      </motion.div>
    </motion.div>,
    container,
  )
}

TooltipBox.displayName = 'TooltipBox'

export default TooltipBox
