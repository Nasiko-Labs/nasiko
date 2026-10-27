import { useChartStable, useYScale } from './chart-context'

/** A dashed horizontal rule at `value` (an average, a budget), labelled at the right edge. */
export function ReferenceLine({ value, label }: { value: number; label: string }) {
  const { innerWidth } = useChartStable()
  const y = useYScale()(value)
  if (y === undefined || !Number.isFinite(y)) return null
  return (
    <g aria-hidden>
      <line
        x1={0}
        x2={innerWidth}
        y1={y}
        y2={y}
        stroke="var(--muted-foreground)"
        strokeDasharray="4 4"
        strokeWidth={1}
      />
      <text
        x={innerWidth}
        y={y - 4}
        textAnchor="end"
        // A halo in the surface colour keeps the label legible over the marks.
        stroke="var(--card)"
        strokeWidth={3}
        paintOrder="stroke"
        className="fill-muted-foreground text-xs"
      >
        {label}
      </text>
    </g>
  )
}
