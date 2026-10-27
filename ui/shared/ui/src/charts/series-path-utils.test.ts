import { curveLinear } from 'd3-shape'
import { expect, test } from 'vitest'
import { computeSeriesPathPoints, seriesPathFromPoints } from './series-path-utils'

test('a null value breaks the line instead of drawing to the top edge', () => {
  const data = [0, 1, 2].map((i) => ({ t: new Date(i * 1000), v: i === 1 ? null : 10 }))
  const pts = computeSeriesPathPoints(
    data,
    (d) => d.t as Date,
    (t) => t.getTime() / 10,
    (v) => 100 - v,
    'v',
  )
  const d = seriesPathFromPoints(pts, curveLinear)
  // Two single-point subpaths, never a segment through y=0.
  expect(d.match(/M/g)).toHaveLength(2)
  expect(d).not.toMatch(/,0(?![\d.])/)
})
