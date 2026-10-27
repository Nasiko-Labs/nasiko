import {
  type FadeEdges,
  fadeGradientStops,
  resolveFadeSides,
  viewportFadeGradientAttrs,
} from './fade-edges'

interface AreaGradientDefsProps {
  gradientId: string
  strokeGradientId: string
  edgeMaskId: string
  edgeGradientId: string
  fill: string
  fillOpacity: number
  gradientToOpacity: number
  /** 0–1: where the bottom stop sits (1 = full-height gradient). */
  gradientSpan?: number
  resolvedStroke: string
  isPatternFill: boolean
  fadeEdges: FadeEdges
  innerWidth: number
  innerHeight: number
}

export function AreaGradientDefs({
  gradientId,
  strokeGradientId,
  edgeMaskId,
  edgeGradientId,
  fill,
  fillOpacity,
  gradientToOpacity,
  gradientSpan = 1,
  resolvedStroke,
  isPatternFill,
  fadeEdges,
  innerWidth,
  innerHeight,
}: AreaGradientDefsProps) {
  const sides = resolveFadeSides(fadeEdges)
  // Stroke gradient mirrors the area's edge fade so the line doesn't pop in
  // past the faded fill. Skip emitting it when neither edge fades — the line
  // can then paint a solid stroke instead of an unnecessary url(#...) ref.
  const strokeStops = sides.any ? fadeGradientStops(sides) : null
  const showEdgeMask = sides.any && !isPatternFill
  const edgeStops = showEdgeMask ? fadeGradientStops(sides) : null
  const span = Math.min(1, Math.max(0.01, gradientSpan))
  const midOffset = `${span * 100}%`

  return (
    <defs>
      {isPatternFill ? null : (
        <linearGradient id={gradientId} x1="0%" x2="0%" y1="0%" y2="100%">
          <stop offset="0%" style={{ stopColor: fill, stopOpacity: fillOpacity }} />
          <stop offset={midOffset} style={{ stopColor: fill, stopOpacity: gradientToOpacity }} />
          {span < 1 ? (
            <stop offset="100%" style={{ stopColor: fill, stopOpacity: gradientToOpacity }} />
          ) : null}
        </linearGradient>
      )}

      {strokeStops ? (
        <linearGradient id={strokeGradientId} {...viewportFadeGradientAttrs(innerWidth)}>
          {strokeStops.map((stop) => (
            <stop
              key={stop.offset}
              offset={stop.offset}
              style={{ stopColor: resolvedStroke, stopOpacity: stop.opacity }}
            />
          ))}
        </linearGradient>
      ) : null}

      {edgeStops ? (
        <>
          <linearGradient id={edgeGradientId} {...viewportFadeGradientAttrs(innerWidth)}>
            {edgeStops.map((stop) => (
              <stop
                key={stop.offset}
                offset={stop.offset}
                style={{ stopColor: 'white', stopOpacity: stop.opacity }}
              />
            ))}
          </linearGradient>
          <mask id={edgeMaskId}>
            <rect
              fill={`url(#${edgeGradientId})`}
              height={innerHeight}
              width={innerWidth}
              x="0"
              y="0"
            />
          </mask>
        </>
      ) : null}
    </defs>
  )
}
