/* eslint-disable react-hooks/refs -- Kit animation code (from the React migration's visx kit): it mounts portals after the
   first render and drives motion springs from values tracked across renders. Correct without the React Compiler,
   so the file opts out of it with 'use no memo'. */
'use client'
'use no memo'

import { motion, useSpring } from 'motion/react'
import { memo, useMemo, useRef } from 'react'

const TICKER_ITEM_HEIGHT = 24
/** Full scroll stacks are skipped above this count — single label + instant updates. */
const COMPACT_TICKER_THRESHOLD = 60

export interface DateTickerProps {
  currentIndex: number
  labels: string[]
  visible: boolean
}

const DateTickerCompact = memo(function DateTickerCompact({
  currentIndex,
  labels,
}: Omit<DateTickerProps, 'visible'>) {
  const label = labels[currentIndex] ?? labels[0] ?? ''

  return (
    <div className="bg-chart-tooltip-background text-chart-tooltip-foreground overflow-hidden rounded-full px-4 py-1 shadow-lg ring-1 ring-black/10 dark:ring-white/10">
      <div className="flex h-6 items-center justify-center">
        <span className="text-sm font-medium whitespace-nowrap">{label}</span>
      </div>
    </div>
  )
})

const DateTickerInner = memo(function DateTickerInner({
  currentIndex,
  labels,
}: Omit<DateTickerProps, 'visible'>) {
  // Parse labels into month and day parts
  const parsedLabels = useMemo(() => {
    return labels.map((label, index) => {
      const parts = label.split(' ')
      const month = parts[0] || ''
      const day = parts[1] || ''
      return { month, day, full: label, key: `${label}::${index}` }
    })
  }, [labels])

  // Month segments: one entry per consecutive run (Jan → Feb → …), keyed by start index
  const monthSegments = useMemo(() => {
    const segments: { month: string; key: string; startIndex: number }[] = []

    parsedLabels.forEach((label, index) => {
      const prev = segments.at(-1)
      if (!prev || prev.month !== label.month) {
        segments.push({
          month: label.month,
          key: `${label.month}-${index}`,
          startIndex: index,
        })
      }
    })

    return segments
  }, [parsedLabels])

  // Index into monthSegments for the current data point
  const currentMonthIndex = useMemo(() => {
    if (currentIndex < 0 || currentIndex >= parsedLabels.length) {
      return 0
    }
    for (let i = monthSegments.length - 1; i >= 0; i--) {
      const segment = monthSegments[i]
      if (segment && segment.startIndex <= currentIndex) {
        return i
      }
    }
    return 0
  }, [currentIndex, parsedLabels.length, monthSegments])

  // Track previous month index
  const prevMonthIndexRef = useRef(-1)

  // Animated Y offsets
  const dayY = useSpring(0, { stiffness: 400, damping: 35 })
  const monthY = useSpring(0, { stiffness: 400, damping: 35 })

  dayY.set(-currentIndex * TICKER_ITEM_HEIGHT)

  if (currentMonthIndex >= 0) {
    const isFirstRender = prevMonthIndexRef.current === -1
    const monthChanged = prevMonthIndexRef.current !== currentMonthIndex
    if (isFirstRender || monthChanged) {
      monthY.set(-currentMonthIndex * TICKER_ITEM_HEIGHT)
      prevMonthIndexRef.current = currentMonthIndex
    }
  }

  return (
    <div className="bg-chart-tooltip-background text-chart-tooltip-foreground overflow-hidden rounded-full px-4 py-1 shadow-lg ring-1 ring-black/10 dark:ring-white/10">
      <div className="relative h-6 overflow-hidden">
        <div className="flex items-center justify-center gap-1">
          {/* Month stack */}
          <div className="relative h-6 overflow-hidden">
            <motion.div className="flex flex-col" style={{ y: monthY }}>
              {monthSegments.map((segment) => (
                <div className="flex h-6 shrink-0 items-center justify-center" key={segment.key}>
                  <span className="text-sm font-medium whitespace-nowrap">{segment.month}</span>
                </div>
              ))}
            </motion.div>
          </div>

          {/* Day stack */}
          <div className="relative h-6 overflow-hidden">
            <motion.div className="flex flex-col" style={{ y: dayY }}>
              {parsedLabels.map((label) => (
                <div className="flex h-6 shrink-0 items-center justify-center" key={label.key}>
                  <span className="text-sm font-medium whitespace-nowrap">{label.day}</span>
                </div>
              ))}
            </motion.div>
          </div>
        </div>
      </div>
    </div>
  )
})

export function DateTicker({ currentIndex, labels, visible }: DateTickerProps) {
  if (!visible || labels.length === 0) {
    return null
  }

  if (labels.length > COMPACT_TICKER_THRESHOLD) {
    return <DateTickerCompact currentIndex={currentIndex} labels={labels} />
  }

  return <DateTickerInner currentIndex={currentIndex} labels={labels} />
}

DateTicker.displayName = 'DateTicker'

export default DateTicker
