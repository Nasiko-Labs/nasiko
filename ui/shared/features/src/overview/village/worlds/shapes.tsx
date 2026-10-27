/**
 * Drawing pieces the world themes share: a dome, an organic ground blob, a gear, and a seeded random for scenery that
 * should look scattered but stay put between renders. Plain SVG in the village's iso space; colours come from the
 * caller or `ART` / `INK`.
 */
import { useContext, useId } from 'react'
import { MotionOn } from '../motionContext'
import { INK, shade } from '../palette'
import { gearPath } from './paths'

/**
 * A dome standing on the ground ellipse centred on (cx, cy) in screen px: rx × ry its base, `h` px tall. Lit from the
 * left like every solid in the village, with a soft highlight.
 */
export function Dome({
  cx,
  cy,
  rx,
  ry,
  h,
  color,
  stroke = true,
}: {
  cx: number
  cy: number
  rx: number
  ry: number
  h: number
  color: string
  stroke?: boolean
}) {
  const id = useId().replace(/:/g, '')
  return (
    <>
      <defs>
        <linearGradient id={`dome${id}`} x1="0" x2="1" y1="0" y2="0">
          <stop offset="0" stopColor={shade(color, 0.16)} />
          <stop offset="0.55" stopColor={color} />
          <stop offset="1" stopColor={shade(color, -0.28)} />
        </linearGradient>
      </defs>
      <path
        d={`M${cx - rx},${cy}A${rx},${h} 0 0 1 ${cx + rx},${cy}A${rx},${ry} 0 0 1 ${cx - rx},${cy}Z`}
        fill={`url(#dome${id})`}
        stroke={stroke ? INK : undefined}
        strokeLinejoin="round"
      />
      <ellipse
        cx={cx - rx * 0.38}
        cy={cy - h * 0.6}
        rx={rx * 0.2}
        ry={h * 0.14}
        fill="#fff"
        opacity={0.3}
      />
    </>
  )
}

/**
 * A gear standing upright on the front-left plane (it faces the viewer's left, like the buildings' lit walls), its
 * hub at (x, y) in screen px, turning while motion is on.
 */
export function UprightGear({
  x,
  y,
  r,
  color,
  teeth = 8,
  dur = 9,
  reverse = false,
  opacity = 1,
  stroke = true,
  plane = 'left',
}: {
  x: number
  y: number
  r: number
  color: string
  teeth?: number
  dur?: number
  reverse?: boolean
  opacity?: number
  stroke?: boolean
  /** Which wall plane it stands in: the front-left (lit) one or the front-right one. */
  plane?: 'left' | 'right'
}) {
  const on = useContext(MotionOn)
  const skew = plane === 'left' ? 'matrix(1,0.5,0,1,0,0)' : 'matrix(1,-0.5,0,1,0,0)'
  return (
    <g transform={`translate(${x.toFixed(1)},${y.toFixed(1)}) ${skew}`} opacity={opacity}>
      <g>
        {on ? (
          <animateTransform
            attributeName="transform"
            type="rotate"
            values={reverse ? '360;0' : '0;360'}
            dur={`${dur}s`}
            repeatCount="indefinite"
          />
        ) : null}
        <path
          d={gearPath(r, teeth)}
          fill={color}
          stroke={stroke ? INK : undefined}
          strokeLinejoin="round"
        />
        <circle r={r * 0.52} fill={shade(color, -0.22)} stroke={stroke ? INK : undefined} />
        <circle r={r * 0.18} fill={shade(color, 0.3)} stroke={stroke ? INK : undefined} />
        {[0, 1, 2, 3].map((k) => (
          <circle
            key={k}
            cx={Math.cos((k * Math.PI) / 2) * r * 0.36}
            cy={Math.sin((k * Math.PI) / 2) * r * 0.36}
            r={r * 0.07}
            fill={INK}
          />
        ))}
      </g>
    </g>
  )
}
