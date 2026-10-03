/**
 * The legend key every chart draws the same way (DESIGN.md "Charts"): the series fill with its 2 px top
 * edge, matching the bars (the visx kit's `Bar`/`SeriesBar` `edge` prop draws the bar side).
 */
import type { Series } from '@/lib/chart'

/** Legend key: the fill with its top edge (a line series passes `line` for a 2 px stroke in its edge). */
export function Swatch({ series, line }: { series: Series; line?: boolean }) {
  if (line)
    return <span aria-hidden className="h-0.5 w-3 shrink-0" style={{ background: series.edge }} />
  return (
    <span
      aria-hidden
      className="size-2.5 shrink-0 rounded-xs"
      style={{ background: series.fill, borderTop: `2px solid ${series.edge}` }}
    />
  )
}
