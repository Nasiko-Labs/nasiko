/**
 * Lab addition (not in the migration kit): reports the hovered row to the chart's owner, so a click on
 * the chart's wrapper can act on it (the kit has no click API). Render it as a chart child.
 *
 * Read the hovered row on the wrapper's `onPointerDown`, not in `onClick`: the kit clears hover on
 * mouse-down (a drag starts a range selection), and pointer-down fires first.
 */
import { useEffect } from 'react'
import { useChartHover, type TooltipData } from './chart-context'

export function ChartHover({ onChange }: { onChange: (hovered: TooltipData | null) => void }) {
  const { tooltipData } = useChartHover()
  useEffect(() => {
    onChange(tooltipData)
  }, [tooltipData, onChange])
  return null
}
ChartHover.displayName = 'ChartHover'
