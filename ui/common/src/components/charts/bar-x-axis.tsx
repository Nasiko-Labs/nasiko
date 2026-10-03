/* eslint-disable react-hooks/refs, react-hooks/set-state-in-effect -- Kit animation code (from the React migration's visx kit): it mounts portals after the
   first render and drives motion springs from values tracked across renders. Correct without the React Compiler,
   so the file opts out of it with 'use no memo'. */
'use client'
'use no memo'

import { motion } from 'motion/react'
import { memo, useEffect, useMemo, useState } from 'react'
import { createPortal } from 'react-dom'
import { cn } from '@/lib/utils'
import { useChart, useChartStable } from './chart-context'

export interface BarXAxisProps {
  /** Width of the date ticker box for fade calculation. Default: 50 */
  tickerHalfWidth?: number
  /** Whether to show all labels or skip some for dense data. Default: false */
  showAllLabels?: boolean
  /** Maximum number of labels to show. Default: 12 */
  maxLabels?: number
}

interface BarXAxisLabelProps {
  label: string
  x: number
  crosshairX: number | null
  isHovering: boolean
  tickerHalfWidth: number
  maxWidth: number
}

function BarXAxisLabel({
  label,
  x,
  crosshairX,
  isHovering,
  tickerHalfWidth,
  maxWidth,
}: BarXAxisLabelProps) {
  const fadeBuffer = 20
  const fadeRadius = tickerHalfWidth + fadeBuffer

  let opacity = 1
  if (isHovering && crosshairX !== null) {
    const distance = Math.abs(x - crosshairX)
    if (distance < tickerHalfWidth) {
      opacity = 0
    } else if (distance < fadeRadius) {
      opacity = (distance - tickerHalfWidth) / fadeBuffer
    }
  }

  // Zero-width container approach for perfect centering
  return (
    <div
      className="absolute"
      style={{
        left: x,
        bottom: 12,
        width: 0,
        display: 'flex',
        justifyContent: 'center',
      }}
    >
      <motion.span
        animate={{ opacity }}
        className={cn('text-chart-label max-w-28 shrink-0 truncate text-xs whitespace-nowrap')}
        initial={{ opacity: 1 }}
        style={{ maxWidth }}
        title={label}
        transition={{ duration: 0.4, ease: 'easeInOut' }}
      >
        {label}
      </motion.span>
    </div>
  )
}

export function BarXAxis(props: BarXAxisProps) {
  const { containerRef, barScale } = useChartStable()
  const [mounted, setMounted] = useState(false)

  useEffect(() => {
    setMounted(true)
  }, [])

  const container = containerRef.current
  if (!(mounted && container)) {
    return null
  }

  if (!barScale) {
    return null
  }

  return <BarXAxisInner {...props} container={container} />
}

const BarXAxisInner = memo(function BarXAxisInner({
  tickerHalfWidth = 50,
  showAllLabels = false,
  maxLabels = 12,
  container,
}: BarXAxisProps & { container: HTMLDivElement }) {
  const { margin, tooltipData, barScale, bandWidth, barXAccessor, data } = useChart()

  // Generate labels for each bar
  const labelsToShow = useMemo(() => {
    if (!(barScale && bandWidth && barXAccessor)) {
      return []
    }

    const allLabels = data.map((d) => {
      const label = barXAccessor(d)
      const bandX = barScale(label) ?? 0
      // Center the label under the bar group
      const x = bandX + bandWidth / 2 + margin.left
      return { label, x }
    })

    // Show every label, or every `step`-th one to avoid crowding. Each shown
    // label may use the room up to the next shown one, less a small gap.
    const step =
      showAllLabels || allLabels.length <= maxLabels ? 1 : Math.ceil(allLabels.length / maxLabels)
    const [first, second] = allLabels
    const pitch = first && second ? second.x - first.x : bandWidth
    const maxWidth = Math.max(24, pitch * step - 6)
    return allLabels.filter((_, i) => i % step === 0).map((l) => ({ ...l, maxWidth }))
  }, [barScale, bandWidth, barXAccessor, data, margin.left, showAllLabels, maxLabels])

  const isHovering = tooltipData !== null
  const crosshairX = tooltipData ? tooltipData.x + margin.left : null

  return createPortal(
    <div className="pointer-events-none absolute inset-0">
      {labelsToShow.map((item) => (
        <BarXAxisLabel
          crosshairX={crosshairX}
          isHovering={isHovering}
          key={`${item.label}-${item.x}`}
          label={item.label}
          maxWidth={item.maxWidth}
          tickerHalfWidth={tickerHalfWidth}
          x={item.x}
        />
      ))}
    </div>,
    container,
  )
})

BarXAxis.displayName = 'BarXAxis'

export default BarXAxis
