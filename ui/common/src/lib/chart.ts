/**
 * Chart series tokens (DESIGN.md "Charts", Two-tone palette). A pastel fill is too light to carry a
 * line, a dot or a thin mark on white, so those draw in the series' `edge`; fills (bars, areas, cells)
 * draw in `fill` with the edge on top. Dark mode sets edge = fill, so nothing changes there.
 */
export interface Series {
  fill: string
  edge: string
}

const SERIES: readonly Series[] = [1, 2, 3, 4, 5].map((n) => ({
  fill: `var(--chart-${n})`,
  edge: `var(--chart-${n}-edge)`,
}))
export const OTHER_SERIES: Series = { fill: 'var(--chart-other)', edge: 'var(--chart-other-edge)' }

/** The i-th series; at most five, so callers fold the rest into Other. */
export const seriesAt = (i: number): Series => SERIES[i % SERIES.length] ?? OTHER_SERIES
