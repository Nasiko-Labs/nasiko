/**
 * Lab: the burn chart (plans/feat-budget-burn.md F1, design 1B, 5C, 7B), on raw visx like the cost scatter: the time
 * series kit draws one series family against a data-bound x domain, and a burn chart needs a fixed period domain, a
 * future band, stepped limits and a hatched area below $0. One card, two plots on one x axis: the burn (burndown or
 * burn-up) on top, the daily bars below, one crosshair, one picked day.
 *
 * Plain data props, no feature imports (`features/budgets/burn.ts` `chartModel` builds them; copy comes as labels).
 * Marks follow the kit's defaults (design 5B): lines in the series edge, the band in the series fill, rules in theme
 * tokens, every mark labelled or in the legend. Pointer-only: the Table view is the keyboard and screen-reader path
 * (design 6B); `summary` is the accessible description. Lines sweep in once, drawn at rest under reduced motion.
 */
import { AxisBottom, AxisLeft } from '@visx/axis'
import { GridRows } from '@visx/grid'
import { Group } from '@visx/group'
import { ParentSize } from '@visx/responsive'
import { scaleLinear, scaleUtc } from '@visx/scale'
import { Area, LinePath } from '@visx/shape'
import { Info } from 'lucide-react'
import { motion, useReducedMotion } from 'motion/react'
import { useId, useState } from 'react'
import { Swatch } from '@nasiko/ui/composites/chart-marks'
import { OTHER_SERIES, seriesAt } from '@nasiko/ui/lib/chart'
import { Tooltip, TooltipContent, TooltipTrigger } from '@nasiko/ui/primitives/tooltip'
import { HatchPattern } from './hatch-pattern'

export interface BurnChartPoint {
  t: number
  v: number
}

export interface BurnChartData {
  mode: 'down' | 'up'
  start: number
  end: number
  actual: BurnChartPoint[]
  pace: BurnChartPoint[][]
  band: { t: number; low: number; high: number }[]
  /** Limit steps; `stops` when calls stopped under that step (drawn as the stop line). In burndown, `v` is 0. */
  limits: { from: number; to: number; v: number; stops: boolean }[]
  thresholds: { from: number; to: number; v: number; percent: number }[]
  runOut: { t: number; observed: boolean } | null
  /** Events merged per day: the flag shows a count past one; hover lists them. */
  markers: { day: string; t: number; labels: string[] }[]
  /** One per day; `parts` stacks it by contributor in `stack` order (keys outside `stack` are not drawn). */
  bars: {
    day: string
    t: number
    v: number
    spike: boolean
    parts?: { key: string; v: number }[]
  }[]
  /** The stack's contributors, largest first; the key `other` takes the Other colour (F3: the top 5 plus Other). */
  stack?: { key: string; label: string }[]
  /** A picked contributor's cumulative share, in the chart's units (F3 share view). */
  share?: BurnChartPoint[]
  minY: number
  maxY: number
}

export interface BurnChartLabels {
  actual: string
  /** A plain-words explanation of `actual`, next to it in the legend as an info icon (bug found 2026-10-26: the
   * legend named every line but explained none of them). Every other `*Hint` below is the same idea. */
  actualHint?: string
  pace: string
  paceHint?: string
  band: string
  bandHint?: string
  limit: string
  limitHint?: string
  threshold: (percent: number) => string
  /** The legend's key for the threshold lines. */
  thresholds: string
  thresholdsHint?: string
  stop: string
  stopHint?: string
  /** "−$41 over", drawn at the line's end when it is below $0 (burndown). */
  over: string | null
  runOut: string | null
  preview: string
  previewHint?: string
  /** Lab: the "expected" line's legend key (F13), when one is drawn. */
  expected?: string
  bars: string
  spike: string
  /** The chart's accessible description: the page's sentence (F12). */
  summary: string
  /** The share line's legend key (the picked contributor's name). */
  share?: string
}

export interface BurnChartProps {
  data: BurnChartData
  labels: BurnChartLabels
  formatMoney: (v: number) => string
  formatDay: (t: number) => string
  /** A dashed "Preview, not applied" line (a lever or what-if). */
  preview?: BurnChartPoint[] | null
  /** Lab: a dotted "expected" line after a change that moved the pace (F13). */
  expected?: BurnChartPoint[] | null
  pickedDay?: string | null
  onPickDay?: (day: string) => void
  className?: string
  /** The burn plot alone, shorter, without the daily bars (Home's Budget burn panel, plan F12). */
  compact?: boolean
  /**
   * Lab: `turns` makes x the session's turns (plan F9): `t` is a turn number (0 the session's start), ticks are whole
   * turns, and `formatDay` labels them. Only with `compact` (a session has no daily bars).
   */
  axis?: 'time' | 'turns'
}

const ACTUAL = seriesAt(0)
const PREVIEW = seriesAt(2)
const EXPECTED = seriesAt(3)
const SHARE = seriesAt(1)
/** A stacked part's colours: the stack's position, Other in the Other colour. */
const partSeries = (stack: readonly { key: string }[], key: string) =>
  key === 'other'
    ? OTHER_SERIES
    : seriesAt(
        Math.max(
          0,
          stack.findIndex((s) => s.key === key),
        ),
      )
const MARGIN = { top: 22, right: 64, bottom: 28, left: 56 }
/** Share of the plot height the burn takes; the daily bars get the rest (design 1B). */
const BURN_SHARE = 0.68
const GAP = 14
const TICK = { fill: 'var(--muted-foreground)', fontSize: 11 }

export function BurnChart(props: BurnChartProps) {
  const { labels, data, preview, expected } = props
  return (
    <figure aria-label={labels.summary} className={props.className}>
      <div role="presentation" className={props.compact ? 'h-56' : 'h-72 md:h-80'}>
        <ParentSize>
          {({ width, height }) =>
            width > 10 && height > 10 ? <Plot {...props} width={width} height={height} /> : null
          }
        </ParentSize>
      </div>
      <figcaption className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
        <Key color={ACTUAL.edge} hint={labels.actualHint}>
          {labels.actual}
        </Key>
        {data.pace.length ? (
          <Key dashed color="var(--muted-foreground)" hint={labels.paceHint}>
            {labels.pace}
          </Key>
        ) : null}
        {data.band.length ? (
          <Key band hint={labels.bandHint}>
            {labels.band}
          </Key>
        ) : null}
        {data.mode === 'up' && data.limits.some((l) => !l.stops) ? (
          <Key color="var(--foreground)" hint={labels.limitHint}>
            {labels.limit}
          </Key>
        ) : null}
        {data.thresholds.length ? (
          <Key dashed color="var(--warning)" hint={labels.thresholdsHint}>
            {labels.thresholds}
          </Key>
        ) : null}
        {data.limits.some((l) => l.stops) ? (
          <Key color="var(--destructive)" hint={labels.stopHint}>
            {labels.stop}
          </Key>
        ) : null}
        {preview?.length ? (
          <Key dashed color={PREVIEW.edge} hint={labels.previewHint}>
            {labels.preview}
          </Key>
        ) : null}
        {expected?.length && labels.expected ? (
          <Key dashed color={EXPECTED.edge}>
            {labels.expected}
          </Key>
        ) : null}
        {data.share?.length && labels.share ? <Key color={SHARE.edge}>{labels.share}</Key> : null}
        {props.compact ? null : data.stack?.length ? (
          data.stack.map((s) => (
            <span key={s.key} className="inline-flex items-center gap-1.5">
              <Swatch series={partSeries(data.stack ?? [], s.key)} /> {s.label}
            </span>
          ))
        ) : (
          <span className="inline-flex items-center gap-1.5">
            <Swatch series={ACTUAL} /> {labels.bars}
          </span>
        )}
      </figcaption>
    </figure>
  )
}

/**
 * A legend key: a solid or dashed rule in the mark's colour, or the band's swatch, with an optional
 * info icon explaining what the line actually means (bug found 2026-10-26: the legend named every
 * line — "Even pace", "Projection from recent spending rates" — but explained none of them).
 */
function Key({
  children,
  color,
  dashed,
  band,
  hint,
}: {
  children: string
  color?: string
  dashed?: boolean
  band?: boolean
  hint?: string
}) {
  return (
    <span className="inline-flex items-center gap-1.5">
      {band ? (
        <span
          aria-hidden
          className="h-2.5 w-3 rounded-xs"
          style={{ background: ACTUAL.fill, opacity: 0.45 }}
        />
      ) : (
        <span
          aria-hidden
          className="w-3"
          style={{ borderTop: `2px ${dashed ? 'dashed' : 'solid'} ${color}` }}
        />
      )}
      {children}
      {hint ? (
        <Tooltip>
          {/* Focusable so a keyboard user can open the tooltip; it has no action of its own. */}
          <TooltipTrigger
            render={
              <span
                role="img"
                // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- focusable only to open the tooltip
                tabIndex={0}
                className="inline-flex text-muted-foreground"
                aria-label={`What “${children}” means`}
              />
            }
          >
            <Info className="size-3" aria-hidden />
          </TooltipTrigger>
          <TooltipContent className="max-w-xs">{hint}</TooltipContent>
        </Tooltip>
      ) : null}
    </span>
  )
}

function Plot({
  data,
  labels,
  formatMoney,
  formatDay,
  preview,
  expected,
  pickedDay,
  onPickDay,
  compact,
  axis,
  width,
  height,
}: BurnChartProps & { width: number; height: number }) {
  const reduce = useReducedMotion()
  const hatch = useId().replace(/:/g, '')
  const [hover, setHover] = useState<number | null>(null)
  const innerW = Math.max(0, width - MARGIN.left - MARGIN.right)
  const innerH = Math.max(0, height - MARGIN.top - MARGIN.bottom)
  const burnH = compact ? innerH : innerH * BURN_SHARE
  const barsTop = burnH + GAP
  const barsH = Math.max(0, innerH - barsTop)

  // UTC, like the day bars and the labels: a local-time scale puts ticks at the viewer's midnight.
  const x = scaleUtc({ domain: [new Date(data.start), new Date(data.end)], range: [0, innerW] })
  // Lab: a session's turns are a linear axis of whole turns (F9).
  const turns = axis === 'turns'
  const xTurn = scaleLinear({ domain: [data.start, data.end], range: [0, innerW] })
  const maxTurnTicks = Math.max(2, Math.min(10, Math.floor(innerW / 60)))
  const turnStep = Math.max(1, Math.ceil((data.end - data.start) / maxTurnTicks))
  const turnTicks: number[] = []
  for (let t = Math.ceil(data.start) || 1; t <= data.end; t += turnStep) turnTicks.push(t)
  const extra = [...(preview ?? []), ...(expected ?? []), ...(data.share ?? [])].map((p) => p.v)
  const lowY = Math.min(0, data.minY, ...extra)
  const highY = Math.max(data.maxY, ...extra) * 1.06 || 1
  // `clamp: true`: a point outside [lowY, highY] (an anomalous day the axis deliberately excludes,
  // `burn.ts` `chartModel`) still draws, pinned to the nearest edge, instead of past the plot area.
  const y = scaleLinear({ domain: [lowY, highY], range: [burnH, 0], nice: true, clamp: true })
  const barMax = Math.max(1e-9, ...data.bars.map((b) => b.v))
  const yb = scaleLinear({ domain: [0, barMax], range: [barsH, 0], nice: true })
  const X = (t: number) => (turns ? xTurn(t) : x(new Date(t)))
  const dayW = Math.max(1, (X(data.start + 86_400_000) - X(data.start)) * 0.72)
  const sweep = reduce ? undefined : { initial: { pathLength: 0 }, animate: { pathLength: 1 } }
  const zeroY = y(0)
  const last = data.actual.at(-1)

  // The day under a point: only a day with a bar, so the projected future picks nothing.
  const dayAt = (t: number | null) =>
    t === null ? null : (data.bars.find((b) => t >= b.t && t < b.t + 86_400_000) ?? null)
  const timeAt = (svg: SVGSVGElement, clientX: number) => {
    const px = clientX - svg.getBoundingClientRect().left - MARGIN.left
    if (px < 0 || px > innerW) return null
    return turns ? xTurn.invert(px) : x.invert(px).getTime()
  }
  const hovered = dayAt(hover)
  const dimOthers = data.bars.some((b) => b.day === pickedDay)
  const atHover = (pts: BurnChartPoint[]) => {
    if (hovered === null) return null
    const t = hovered.t + 86_400_000
    // A day's closing point: at a reset the old period's close comes before the new period's $0 at the same instant.
    return pts.find((q) => q.t === t) ?? pts.findLast((q) => q.t <= t) ?? null
  }

  const actualAt = atHover(data.actual)

  return (
    <svg
      width={width}
      height={height}
      className="overflow-visible"
      onPointerMove={(e) => setHover(timeAt(e.currentTarget, e.clientX))}
      onPointerLeave={() => setHover(null)}
      // From the click itself: a touch tap has no pointermove, and pointerleave clears the hover first.
      onClick={(e) => {
        const day = dayAt(timeAt(e.currentTarget, e.clientX))
        if (day && onPickDay) onPickDay(day.day)
      }}
    >
      <HatchPattern id={hatch} stroke="var(--destructive)" />
      <Group left={MARGIN.left} top={MARGIN.top}>
        <GridRows
          scale={y}
          width={innerW}
          numTicks={4}
          stroke="var(--border)"
          strokeOpacity={0.6}
        />
        {/* Over the limit: the area below $0 is hatched with a destructive edge (design 5C). */}
        {data.mode === 'down' && lowY < 0 ? (
          <g aria-hidden>
            <rect
              x={0}
              y={zeroY}
              width={innerW}
              height={Math.max(0, burnH - zeroY)}
              fill={`url(#${hatch})`}
              opacity={0.25}
            />
            <line
              x1={0}
              x2={innerW}
              y1={zeroY}
              y2={zeroY}
              stroke="var(--destructive)"
              strokeWidth={1}
            />
          </g>
        ) : null}
        {/* The band from as_of to the reset (design 4B: "projection from recent spending rates"). */}
        {data.band.length ? (
          <g aria-hidden>
            <Area
              data={data.band}
              x={(d) => X(d.t)}
              y0={(d) => y(d.low)}
              y1={(d) => y(d.high)}
              fill={ACTUAL.fill}
              fillOpacity={0.45}
            />
            {/* The edges, so a band whose two rates agree still shows its projection as one dashed line. */}
            {(['low', 'high'] as const).map((edge) => (
              <LinePath
                key={edge}
                data={data.band}
                x={(d) => X(d.t)}
                y={(d) => y(d[edge])}
                stroke={ACTUAL.edge}
                strokeWidth={1.5}
                strokeDasharray="5 4"
              />
            ))}
          </g>
        ) : null}
        {data.pace.map((seg) => (
          <LinePath
            key={seg[0]?.t ?? 0}
            data={seg}
            x={(d) => X(d.t)}
            y={(d) => y(d.v)}
            stroke="var(--muted-foreground)"
            strokeDasharray="4 4"
            strokeWidth={1}
            aria-hidden
          />
        ))}
        {data.mode === 'up'
          ? data.limits.map((l, i) => (
              <g key={l.from} aria-hidden>
                <line
                  x1={X(l.from)}
                  x2={X(l.to)}
                  y1={y(l.v)}
                  y2={y(l.v)}
                  stroke={l.stops ? 'var(--destructive)' : 'var(--foreground)'}
                  strokeWidth={1.5}
                />
                {/* Inside the plot, above the line, right-aligned (as ReferenceLine): never clipped by the margin. */}
                {i === data.limits.length - 1 ? (
                  <Label x={X(l.to) - 2} y={y(l.v) - 9} anchor="end">
                    {l.stops ? `${formatMoney(l.v)} · ${labels.stop}` : formatMoney(l.v)}
                  </Label>
                ) : null}
              </g>
            ))
          : null}
        {data.thresholds.map((th) => (
          <g key={`${th.from}:${th.percent}`} aria-hidden>
            <line
              x1={X(th.from)}
              x2={X(th.to)}
              y1={y(th.v)}
              y2={y(th.v)}
              stroke="var(--warning)"
              strokeDasharray="2 3"
              strokeWidth={1}
            />
            {th.to >= data.end - 1 ? (
              <Label x={X(th.to) + 4} y={y(th.v)} anchor="start">
                {labels.threshold(th.percent)}
              </Label>
            ) : null}
          </g>
        ))}
        {/* Burndown: calls stop at $0 left, only across the steps that stopped them. */}
        {data.mode === 'down'
          ? data.limits.map((l, i) =>
              l.stops ? (
                <g key={l.from} aria-hidden>
                  <line
                    x1={X(l.from)}
                    x2={X(l.to)}
                    y1={zeroY}
                    y2={zeroY}
                    stroke="var(--destructive)"
                    strokeWidth={1.5}
                  />
                  {i === data.limits.length - 1 ? (
                    <Label x={X(l.to) + 4} y={zeroY} anchor="start">
                      {labels.stop}
                    </Label>
                  ) : null}
                </g>
              ) : null,
            )
          : null}
        {preview?.length ? (
          <LinePath
            data={preview}
            x={(d) => X(d.t)}
            y={(d) => y(d.v)}
            stroke={PREVIEW.edge}
            strokeWidth={2}
            strokeDasharray="6 4"
            aria-hidden
          />
        ) : null}
        {expected?.length ? (
          <LinePath
            data={expected}
            x={(d) => X(d.t)}
            y={(d) => y(d.v)}
            stroke={EXPECTED.edge}
            strokeWidth={2}
            strokeDasharray="2 4"
            aria-hidden
          />
        ) : null}
        {data.share?.length ? (
          <LinePath
            data={data.share}
            x={(d) => X(d.t)}
            y={(d) => y(d.v)}
            stroke={SHARE.edge}
            strokeWidth={2}
            aria-hidden
          />
        ) : null}
        <LinePath data={data.actual} x={(d) => X(d.t)} y={(d) => y(d.v)}>
          {({ path }) => (
            <motion.path
              d={path(data.actual) ?? ''}
              fill="none"
              stroke={ACTUAL.edge}
              strokeWidth={2.5}
              strokeLinejoin="round"
              transition={{ duration: 0.6, ease: 'easeOut' }}
              aria-hidden
              {...sweep}
            />
          )}
        </LinePath>
        {last ? (
          <g aria-hidden>
            <circle cx={X(last.t)} cy={y(last.v)} r={3.5} fill={ACTUAL.edge} />
            {labels.over ? (
              <Label x={X(last.t) + 6} y={y(last.v) + 12} anchor="start" tone="var(--destructive)">
                {labels.over}
              </Label>
            ) : null}
          </g>
        ) : null}
        {data.runOut && labels.runOut ? (
          <g aria-hidden>
            <line
              x1={X(data.runOut.t)}
              x2={X(data.runOut.t)}
              y1={0}
              y2={burnH}
              stroke="var(--destructive)"
              strokeDasharray={data.runOut.observed ? undefined : '3 3'}
              strokeWidth={1}
            />
            <Label x={X(data.runOut.t)} y={-8} anchor="middle" tone="var(--destructive)">
              {labels.runOut}
            </Label>
          </g>
        ) : null}
        {/* Event flags, merged per day with a count (design 7B); hover lists them. */}
        {data.markers.map((m) => (
          <g key={m.day} transform={`translate(${X(m.t)}, ${burnH - 2})`} aria-hidden>
            <title>{m.labels.join('\n')}</title>
            <path
              d="M0,0 L0,-12 L8,-9 L0,-6"
              fill="none"
              stroke="var(--foreground)"
              strokeWidth={1.2}
            />
            {/* The count of merged events, as the hub's rail shows it ("+3" for three; plan F1). */}
            {m.labels.length > 1 ? (
              <Label x={4} y={-19} anchor="middle">
                {`+${m.labels.length}`}
              </Label>
            ) : null}
          </g>
        ))}

        {/* The daily bars, on the same x axis. */}
        {compact ? (
          <AxisBottom
            top={burnH}
            scale={turns ? xTurn : x}
            numTicks={Math.max(2, Math.min(8, Math.floor(innerW / 90)))}
            tickValues={turns ? turnTicks : undefined}
            tickFormat={(d) => formatDay(turns ? Number(d) : (d as Date).getTime())}
            stroke="var(--border)"
            tickStroke="var(--border)"
            tickLabelProps={() => ({ ...TICK, textAnchor: 'middle' })}
          />
        ) : (
          <Group top={barsTop}>
            {data.bars.map((b) => {
              const h = barsH - yb(b.v)
              const picked = b.day === pickedDay
              const opacity = dimOthers && !picked ? 0.45 : 1
              const left = X(b.t + 43_200_000) - dayW / 2
              const stack = data.stack ?? []
              let base = 0
              return (
                <g key={b.day} aria-hidden>
                  {b.parts && stack.length ? (
                    // Bottom up in stack order; each part's top edge in its own colour.
                    stack.map((s) => {
                      const v = b.parts?.find((p) => p.key === s.key)?.v ?? 0
                      if (v <= 0) return null
                      const y0 = yb(base)
                      base += v
                      const y1 = yb(base)
                      const series = partSeries(stack, s.key)
                      return (
                        <g key={s.key} opacity={opacity}>
                          <rect
                            x={left}
                            y={y1}
                            width={dayW}
                            height={Math.max(0, y0 - y1)}
                            fill={series.fill}
                          />
                          <rect x={left} y={y1} width={dayW} height={1.5} fill={series.edge} />
                        </g>
                      )
                    })
                  ) : (
                    <rect
                      x={left}
                      y={yb(b.v)}
                      width={dayW}
                      height={Math.max(0, h)}
                      fill={ACTUAL.fill}
                      opacity={opacity}
                    />
                  )}
                  {h > 0 && !(b.parts && stack.length) ? (
                    <rect
                      x={X(b.t + 43_200_000) - dayW / 2}
                      y={yb(b.v)}
                      width={dayW}
                      height={2}
                      fill={ACTUAL.edge}
                    />
                  ) : null}
                  {b.spike ? (
                    <text
                      x={X(b.t + 43_200_000)}
                      y={yb(b.v) - 3}
                      textAnchor="middle"
                      className="fill-warning text-2xs font-semibold"
                    >
                      !
                    </text>
                  ) : null}
                </g>
              )
            })}
            <AxisLeft
              scale={yb}
              numTicks={2}
              tickFormat={(v) => formatMoney(Number(v))}
              stroke="transparent"
              tickStroke="transparent"
              tickLabelProps={() => ({ ...TICK, textAnchor: 'end', dx: -4, dy: 3 })}
            />
            <AxisBottom
              top={barsH}
              scale={x}
              numTicks={Math.max(2, Math.min(8, Math.floor(innerW / 90)))}
              tickFormat={(d) => formatDay((d as Date).getTime())}
              stroke="var(--border)"
              tickStroke="var(--border)"
              tickLabelProps={() => ({ ...TICK, textAnchor: 'middle' })}
            />
          </Group>
        )}
        <AxisLeft
          scale={y}
          numTicks={4}
          tickFormat={(v) => formatMoney(Number(v))}
          stroke="transparent"
          tickStroke="transparent"
          tickLabelProps={() => ({ ...TICK, textAnchor: 'end', dx: -4, dy: 3 })}
        />

        {/* One crosshair across both plots, and the picked day's outline. */}
        {hovered ? (
          <line
            aria-hidden
            x1={X(hovered.t + 43_200_000)}
            x2={X(hovered.t + 43_200_000)}
            y1={0}
            y2={innerH}
            stroke="var(--muted-foreground)"
            strokeWidth={1}
          />
        ) : null}
      </Group>
      {hovered ? (
        <Tip
          left={Math.min(width - 210, MARGIN.left + X(hovered.t + 43_200_000) + 10)}
          top={MARGIN.top}
          title={formatDay(hovered.t)}
          rows={[
            [labels.bars, formatMoney(hovered.v)],
            ...(hovered.parts && data.stack
              ? data.stack.flatMap((s): [string, string][] => {
                  const v = hovered.parts?.find((p) => p.key === s.key)?.v ?? 0
                  return v > 0 ? [[s.label, formatMoney(v)]] : []
                })
              : []),
            ...(actualAt ? [[labels.actual, formatMoney(actualAt.v)] as [string, string]] : []),
            ...(hovered.spike ? [[labels.spike, ''] as [string, string]] : []),
          ]}
        />
      ) : null}
    </svg>
  )
}

function Label({
  x,
  y,
  anchor,
  tone = 'var(--muted-foreground)',
  children,
}: {
  x: number
  y: number
  anchor: 'start' | 'middle' | 'end'
  tone?: string
  children: string
}) {
  return (
    <text
      x={x}
      y={y}
      dy={3}
      textAnchor={anchor}
      fill={tone}
      // A halo in the surface colour keeps the label legible over the marks (as ReferenceLine).
      stroke="var(--card)"
      strokeWidth={3}
      paintOrder="stroke"
      className="text-2xs"
    >
      {children}
    </text>
  )
}

function Tip({
  left,
  top,
  title,
  rows,
}: {
  left: number
  top: number
  title: string
  rows: [string, string][]
}) {
  return (
    <foreignObject x={left} y={top} width={200} height={24 + rows.length * 18} aria-hidden>
      <div className="rounded-md border border-border bg-popover px-2 py-1 text-xs text-popover-foreground shadow-sm">
        <div className="font-medium">{title}</div>
        {rows.map(([k, v]) => (
          <div key={k} className="flex justify-between gap-3 tabular-nums">
            <span className="truncate text-muted-foreground">{k}</span>
            <span>{v}</span>
          </div>
        ))}
      </div>
    </foreignObject>
  )
}
