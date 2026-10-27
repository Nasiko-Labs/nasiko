/**
 * The swimlane timeline (plans/feat-flows.md F2, F3, F10, F13, F17, F21-F23, F25, F27, F28; sizes S1).
 *
 * One lane per agent on a shared time axis, the Orchestrator's own lane on top for routed flows. A bar shows a short
 * task label and a muted duration; status shows on a bar only when it isn't OK (failed, waiting, running). Parallel
 * calls are overlapping bars (sub-rows within one agent's lane) on a tinted "N in parallel" band. Colours are
 * DESIGN.md's Two-tone series per agent by first call (F21); waits are hatched; a long wait collapses into a labelled
 * break (F28). Lanes are zebra bands with faint tick lines. Soft curves under the bars join a caller to each call it
 * made; on the critical path (F22) they take the info colour and the bars off it dim (F30: the Carbon accent is near-black).
 *
 * Keyboard and screen readers (F25): every bar is a button with its whole story as its name and in a tooltip (so a
 * bar too narrow for text still says it); the chart is one tab stop (roving tabindex): ←/→ in start order, ↑/↓ across
 * lanes, Home/End, Enter selects (`?step=`) and Enter again clears it, as does Esc. Selection is a thick edge and bold
 * text: never the focus ring (that's focus) or the critical path's outline.
 */
import { Hand, X } from 'lucide-react'
import { useEffect, useRef, useState, type KeyboardEvent } from 'react'
import { Button } from '@nasiko/ui/primitives/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@nasiko/ui/primitives/tooltip'
import { OTHER_SERIES, seriesAt } from '@nasiko/ui/lib/chart'
import { cn } from '@nasiko/ui/lib/utils'
import type { Call } from '../calls'
import { copy } from '../copy'
import { barLabel, taskOf } from '../barLabel'
import { moveFrom, type Move } from '../keys'
import type { Lane, LaneModel } from '../lanes'
import { fmtDuration, fmtWait } from '../precision'
import {
  BREAK_PX,
  cssAt,
  curve,
  position,
  span,
  ticks,
  type Axis,
  type Edge,
  type FanOut,
} from '../timeline'

/** S1: lane 32 px, bars 20 px, sub-rows 24 px; the label column is as wide as a name needs, up to 14 rem (F30). */
const AXIS_PX = 24
const ROW_PX = 24
const LANE_MIN_PX = 32
const BAR_PX = 20
const BRACKET_PX = 14
/** Calls starting this close together share one trunk. */
const TRUNK_PX = 24
const GRID = 'grid grid-cols-[minmax(8rem,14rem)_1fr] gap-x-3 max-sm:grid-cols-[6rem_1fr]'
/** The label column stays in place while a phone scrolls the time area (F24). */
const STICKY = 'max-sm:sticky max-sm:left-0 max-sm:z-10 max-sm:bg-card'
const KEYS: Record<string, Move> = {
  ArrowLeft: 'prev',
  ArrowRight: 'next',
  ArrowUp: 'up',
  ArrowDown: 'down',
  Home: 'first',
  End: 'last',
}

export interface SwimlaneProps {
  model: LaneModel
  axis: Axis
  now: number
  /** The Orchestrator's own lane (routed flows): from the flow's start to its end, or now. */
  orchestrator: { startMs: number; endMs: number | null } | null
  fanOuts: readonly FanOut[]
  /** Calls on the critical path, when it's proven and switched on (F22). */
  critical: ReadonlySet<string> | null
  label: string
  /** The selected call's key (`?step=`). */
  selected: string | null
  onSelect: (key: string | null) => void
}

const laneHeight = (lane: Lane, bracket: boolean) =>
  Math.max(LANE_MIN_PX, (bracket ? BRACKET_PX : 0) + lane.rows.length * ROW_PX + 8)

/** A bar's top inside its lane. */
const barTop = (lane: Lane, row: number, bracket: boolean) => {
  const top = bracket ? BRACKET_PX : 0
  return lane.rows.length === 1 && !bracket ? (LANE_MIN_PX - BAR_PX) / 2 : top + 4 + row * ROW_PX
}

export function Swimlane({
  model,
  axis,
  now,
  orchestrator,
  fanOuts,
  critical,
  label,
  selected,
  onSelect,
}: SwimlaneProps) {
  const [showAll, setShowAll] = useState(false)
  const lanes = showAll ? [...model.lanes, ...model.hidden] : model.lanes
  const laneOf = new Map<string, string>()
  for (const l of [...model.lanes, ...model.hidden])
    for (const c of [...l.rows.flat(), ...l.overflow]) laneOf.set(c.key, l.id)
  // A bracket sits on the lane of its first call.
  const bracketsBy = new Map<string, FanOut[]>()
  for (const f of fanOuts) {
    const lane = f.keys[0] ? laneOf.get(f.keys[0]) : undefined
    if (lane) bracketsBy.set(lane, [...(bracketsBy.get(lane) ?? []), f])
  }

  // Roving focus over the drawn bars.
  const drawn = lanes.map((l) => l.rows.flat())
  const keys = new Set(drawn.flat().map((c) => c.key))
  const [active, setActive] = useState<string | null>(null)
  const current =
    (active && keys.has(active) ? active : null) ??
    (selected && keys.has(selected) ? selected : null) ??
    moveFrom(drawn, null, 'first')
  const bars = useRef(new Map<string, HTMLButtonElement>())
  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    if (e.key === 'Escape') {
      onSelect(null)
      return
    }
    const move = KEYS[e.key]
    if (!move) return
    e.preventDefault()
    const next = moveFrom(drawn, current, move)
    if (!next) return
    setActive(next)
    bars.current.get(next)?.focus()
  }

  // Where each drawn bar's centre sits, for the connectors.
  const centre = new Map<string, { y: number; call: Call }>()
  let y = AXIS_PX + (orchestrator ? LANE_MIN_PX : 0)
  for (const lane of lanes) {
    const b = bracketsBy.has(lane.id)
    lane.rows.forEach((row, i) => {
      for (const c of row) centre.set(c.key, { y: y + barTop(lane, i, b) + BAR_PX / 2, call: c })
    })
    y += laneHeight(lane, b)
  }
  const path = critical
    ? [...critical]
        .flatMap((k) => centre.get(k) ?? [])
        .sort((a, b) => a.call.startMs - b.call.startMs)
    : []
  // Caller → call links (muted), and consecutive calls on the critical path (accent), one curve per pair.
  const edges = new Map<string, Edge>()
  for (const { call } of centre.values()) {
    const from = call.parentKey ? centre.get(call.parentKey) : undefined
    const to = centre.get(call.key)
    if (from && to) edges.set(call.key, { from, to, onPath: false })
  }
  // A call the critical path reaches is joined by that link alone: one line into each bar.
  path.slice(1).forEach((to, i) => {
    const from = path[i]
    if (from) edges.set(to.call.key, { from, to, onPath: true })
  })

  // The curves are drawn in pixels: the track's width is measured (none until it is, e.g. in jsdom).
  const trackRef = useRef<HTMLDivElement>(null)
  const [trackPx, setTrackPx] = useState(0)
  useEffect(() => {
    const el = trackRef.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(([entry]) => setTrackPx(entry?.contentRect.width ?? 0))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  const xAt = (t: number) => {
    const p = position(axis, t)
    return (trackPx - (axis.gap ? BREAK_PX : 0)) * p.frac + p.px
  }
  // Calls a caller made together (starts within TRUNK_PX) branch off one trunk, just left of the first of them; a
  // call made later drops on its own line instead of running a long elbow back to that trunk.
  const trunk = new Map<string, number>()
  const drops = [...edges.values()]
    .filter((e) => xAt(e.from.call.endMs ?? now) > xAt(e.to.call.startMs) + 1)
    .sort((a, b) => a.to.call.startMs - b.to.call.startMs)
  const lastTrunk = new Map<string, { bx: number; x: number }>()
  for (const e of drops) {
    const bx = xAt(e.to.call.startMs)
    const prev = lastTrunk.get(e.from.call.key)
    const x =
      prev && bx - prev.bx <= TRUNK_PX ? prev.x : Math.max(xAt(e.from.call.startMs) + 4, bx - 8)
    if (!prev || prev.x !== x) lastTrunk.set(e.from.call.key, { bx, x })
    trunk.set(e.to.call.key, x)
  }

  return (
    <div className="flex flex-col gap-1">
      {/* One tab stop (the current bar); arrows move between bars. Named by the flow's summary. */}
      {/* Phones (F24): the time area scrolls sideways inside the panel, never the page; the labels stay put. */}
      <div className="max-sm:overflow-x-auto">
        <div
          role="group"
          aria-label={label}
          className={cn('relative flex flex-col max-sm:min-w-160', BAR_TINT)}
        >
          <AxisRow axis={axis} />
          {orchestrator ? (
            <div className={cn(GRID, STRIPE)}>
              <LaneLabel
                name={copy.panel.orchestrator}
                indent={0}
                colour={null}
                top={(LANE_MIN_PX - BAR_PX) / 2}
              />
              <Track axis={axis} height={LANE_MIN_PX}>
                {/* The Orchestrator isn't a task, it spans the flow: a dimension line (├──┤), not a bar. Its name is the lane's. */}
                <div
                  aria-hidden
                  className="absolute top-1/2 z-2 h-3 -translate-y-1/2 border-x-2"
                  style={{
                    ...span(axis, orchestrator.startMs, orchestrator.endMs ?? now),
                    borderColor: 'var(--chart-other-edge)',
                  }}
                >
                  <span
                    className="absolute inset-x-0 top-1/2 h-0.5 -translate-y-1/2 rounded-full"
                    style={{ backgroundColor: 'var(--chart-other-edge)' }}
                  />
                </div>
              </Track>
            </div>
          ) : null}
          {lanes.map((lane, i) => (
            <LaneRow
              key={lane.id}
              stripe={(i + (orchestrator ? 1 : 0)) % 2 === 0}
              lane={lane}
              axis={axis}
              now={now}
              brackets={bracketsBy.get(lane.id) ?? []}
              fromCaller={model.fromCaller}
              critical={critical}
              selected={selected}
              current={current}
              onSelect={(key) => {
                setActive(key)
                // A pressed bar toggles off, as its aria-pressed promises.
                onSelect(selected === key ? null : key)
              }}
              onKeyDown={onKeyDown}
              refFor={(key) => (el) => {
                if (el) bars.current.set(key, el)
                else bars.current.delete(key)
              }}
            />
          ))}
          {/* Above the lane bands and parallel bands, under the bars (z-2), which are opaque: no curve crosses text. */}
          <div aria-hidden className={cn(GRID, 'pointer-events-none absolute inset-0 z-1')}>
            <div />
            <div ref={trackRef} className="relative">
              {trackPx > 0 && edges.size ? (
                <svg className="absolute inset-0 size-full overflow-visible" fill="none">
                  {[...edges].map(([key, e]) => (
                    <path
                      key={key}
                      data-edge={e.onPath ? 'critical' : 'call'}
                      d={curve(e, xAt, now, BAR_PX, trunk.get(e.to.call.key))}
                      stroke={e.onPath ? 'var(--info)' : 'var(--chart-other-edge)'}
                      strokeOpacity={e.onPath ? 1 : 0.7}
                      strokeWidth={e.onPath ? 1.75 : 1.25}
                      strokeLinecap="round"
                    />
                  ))}
                </svg>
              ) : null}
            </div>
          </div>
        </div>
      </div>
      {model.hidden.length ? (
        <div className={GRID}>
          <div>
            <Button
              variant="ghost"
              size="sm"
              className="pointer-coarse:min-h-11"
              onClick={() => setShowAll((v) => !v)}
            >
              {showAll ? copy.panel.hideLanes : copy.panel.showLanes(model.hidden.length)}
            </Button>
          </div>
        </div>
      ) : null}
    </div>
  )
}

function AxisRow({ axis }: { axis: Axis }) {
  return (
    <div className={cn(GRID, 'h-6 border-b border-border')} aria-hidden>
      <div className={STICKY} />
      <div className="relative text-2xs text-muted-foreground tabular-nums">
        {ticks(axis).map((t) => (
          <span
            key={t.ms}
            className="absolute top-1 -translate-x-1/2 whitespace-nowrap first:translate-x-0 last:-translate-x-full max-sm:even:hidden"
            style={{ left: t.left }}
          >
            {t.offsetMs === 0 ? '0' : `+${fmtDuration(t.offsetMs)}`}
          </span>
        ))}
        {axis.gap ? (
          <span
            className="absolute top-1 z-10 bg-card px-1 whitespace-nowrap"
            // A wait still open runs to the right edge: the label hangs left of it instead of off the panel.
            style={
              position(axis, axis.gap.toMs).frac >= 0.999
                ? { right: 0 }
                : { left: cssAt(axis, position(axis, axis.gap.fromMs)) }
            }
          >
            {copy.panel.waiting(fmtWait(axis.gap.toMs - axis.gap.fromMs))}
          </span>
        ) : null}
      </div>
    </div>
  )
}

/** A lane's name sits level with its first bar (a 20 px line on a 20 px bar), never centred in a tall lane. */
function LaneLabel({
  name,
  indent,
  colour,
  top,
}: {
  name: string
  indent: number
  colour: number | null
  top: number
}) {
  const s = colour === null ? OTHER_SERIES : seriesAt(colour)
  return (
    <div
      aria-hidden
      className={cn('flex h-full min-w-0 items-start gap-2 pl-2', STICKY)}
      style={{ paddingLeft: 8 + indent * 12, paddingTop: top }}
    >
      <span className="mt-1.5 size-2 shrink-0 rounded-full" style={{ backgroundColor: s.edge }} />
      <span className="truncate text-sm leading-5">{name}</span>
    </div>
  )
}

function Track({
  axis,
  height,
  children,
}: {
  axis: Axis
  height: number
  children: React.ReactNode
}) {
  return (
    <div className="relative" style={{ height }}>
      {ticks(axis).map((t) => (
        <div
          key={t.ms}
          aria-hidden
          className="absolute inset-y-0 w-px bg-border/60"
          style={{ left: t.left }}
        />
      ))}
      {axis.gap ? (
        <div
          aria-hidden
          className="absolute inset-y-0 bg-muted/60"
          style={{ left: cssAt(axis, position(axis, axis.gap.fromMs)), width: BREAK_PX }}
        />
      ) : null}
      {children}
    </div>
  )
}

function LaneRow({
  stripe,
  lane,
  axis,
  now,
  brackets,
  fromCaller,
  critical,
  selected,
  current,
  onSelect,
  onKeyDown,
  refFor,
}: {
  stripe: boolean
  lane: Lane
  axis: Axis
  now: number
  brackets: readonly FanOut[]
  fromCaller: ReadonlyMap<string, string>
  critical: ReadonlySet<string> | null
  selected: string | null
  current: string | null
  onSelect: (key: string) => void
  onKeyDown: (e: KeyboardEvent<HTMLButtonElement>) => void
  refFor: (key: string) => (el: HTMLButtonElement | null) => void
}) {
  const hasBracket = brackets.length > 0
  return (
    <div className={cn(GRID, stripe && STRIPE)}>
      <LaneLabel
        name={lane.agentName ?? copy.panel.unknownAgent}
        indent={lane.indent}
        colour={lane.colour}
        top={barTop(lane, 0, hasBracket)}
      />
      <Track axis={axis} height={laneHeight(lane, hasBracket)}>
        {/* Parallel calls share a tinted band, its chip above the bars: nothing crosses a bar's label. */}
        {brackets.map((b) => (
          <div
            key={b.keys.join()}
            aria-hidden
            className="absolute inset-y-1 rounded-md bg-muted/70"
            style={span(axis, b.fromMs, b.toMs)}
          >
            <span className="absolute top-0 left-1.5 text-2xs leading-[14px] font-medium whitespace-nowrap text-muted-foreground">
              {copy.panel.parallel(b.peak)}
            </span>
          </div>
        ))}
        {lane.rows.map((row, i) =>
          row.map((c) => (
            <Bar
              key={c.key}
              call={c}
              axis={axis}
              now={now}
              top={barTop(lane, i, hasBracket)}
              colour={lane.colour}
              from={fromCaller.get(c.key) ?? null}
              critical={critical}
              selected={selected === c.key}
              tabbable={current === c.key}
              onSelect={() => onSelect(c.key)}
              onKeyDown={onKeyDown}
              ref={refFor(c.key)}
            />
          )),
        )}
        {lane.overflow.length ? (
          <span className="absolute right-1 bottom-1 text-2xs text-muted-foreground">
            {copy.panel.overflow(lane.overflow.length)}
          </span>
        ) : null}
      </Track>
    </div>
  )
}

function Bar({
  call,
  axis,
  now,
  top,
  colour,
  from,
  critical,
  selected,
  tabbable,
  onSelect,
  onKeyDown,
  ref,
}: {
  call: Call
  axis: Axis
  now: number
  top: number
  colour: number | null
  from: string | null
  critical: ReadonlySet<string> | null
  selected: boolean
  tabbable: boolean
  onSelect: () => void
  onKeyDown: (e: KeyboardEvent<HTMLButtonElement>) => void
  ref: (el: HTMLButtonElement | null) => void
}) {
  const s = colour === null ? OTHER_SERIES : seriesAt(colour)
  const failed = call.status === 'failed'
  const waiting = call.status === 'awaiting_human'
  const open = call.endMs === null && !waiting
  const pending = call.status === 'pending'
  const end = call.endMs ?? now
  // A call waiting on a human worked until it paused: that part is drawn as work, the hatched bar is the wait.
  const pausedAt =
    waiting &&
    call.waitStartMs !== null &&
    call.waitStartMs > call.startMs &&
    call.waitStartMs < end
      ? call.waitStartMs
      : null
  const onPath = critical?.has(call.key) ?? false
  const fill = tint(s.fill, pending ? '10%' : critical && !onPath ? 'var(--bar-dim)' : 'var(--bar)')
  const task = taskOf(call)
  const name = barLabel(call, from, onPath)
  return (
    <>
      {pausedAt !== null ? (
        <div
          aria-hidden
          data-part="work"
          className="pointer-events-none absolute z-2 flex h-5 justify-end overflow-hidden rounded-l-sm border-l-2 px-1 text-2xs leading-5 font-medium whitespace-nowrap"
          style={{
            top,
            ...span(axis, call.startMs, pausedAt),
            backgroundColor: 'var(--card)',
            backgroundImage: `linear-gradient(${tint(s.fill, 'var(--bar)')}, ${tint(s.fill, 'var(--bar)')})`,
            borderLeftColor: s.edge,
          }}
        >
          {/* The wait after it is often a narrow 48 px break: its words sit here, against the hatch. */}
          {copy.panel.waitingOnYou}
        </div>
      ) : null}
      <Tooltip>
        <TooltipTrigger
          render={
            <Button
              ref={ref}
              type="button"
              variant="ghost"
              tabIndex={tabbable ? 0 : -1}
              aria-label={name}
              aria-pressed={selected}
              data-call={call.key}
              onClick={onSelect}
              onKeyDown={onKeyDown}
              className={cn(
                // A narrow bar keeps its drawn width; on coarse pointers its hit area grows (F25).
                'absolute z-2 flex h-5 min-w-0.5 justify-start gap-1 overflow-hidden rounded-sm border-l-2 px-1 py-0 text-2xs leading-none font-normal whitespace-nowrap shadow-none hover:brightness-95',
                "pointer-coarse:after:absolute pointer-coarse:after:inset-x-0 pointer-coarse:after:-inset-y-3 pointer-coarse:after:content-['']",
                pending && 'border border-dashed',
                call.maybeSame && 'outline-1 outline-muted-foreground outline-dashed',
                // Selection is its own mark (a thick edge and bold text), never the focus ring or the path's outline.
                selected && 'border-l-4 font-semibold',
              )}
              style={{
                top,
                ...span(axis, pausedAt ?? call.startMs, end),
                // Opaque (a tint over the card) so the curves under it never cross its text.
                backgroundColor: 'var(--card)',
                backgroundImage: waiting
                  ? 'repeating-linear-gradient(45deg, color-mix(in oklab, var(--warning) 35%, transparent) 0 2px, transparent 2px 6px)'
                  : // Off the critical path only the fill dims: the 11 px label keeps its contrast.
                    `linear-gradient(${fill}, ${fill})`,
                borderLeftColor: failed
                  ? 'var(--destructive)'
                  : waiting
                    ? 'var(--warning)'
                    : s.edge,
              }}
            />
          }
        >
          {/* Text on a tinted bar stays foreground (4.5:1 at 11 px); the X and the red edge carry the colour. */}
          {failed ? <X className="size-3 shrink-0 text-destructive" aria-hidden /> : null}
          {failed ? <span className="font-medium">{copy.stepStatus.failed}</span> : null}
          {waiting && pausedAt !== null ? (
            <Hand className="size-3 shrink-0 text-foreground" aria-hidden />
          ) : waiting ? (
            <span className="font-medium">{copy.panel.waitingOnYou}</span>
          ) : null}
          {/* A paused call's hatch is just the wait: its task stays in the tooltip and the accessible name. */}
          {task && pausedAt === null ? <span className="truncate">{task}</span> : null}
          {from && pausedAt === null ? <span>· {copy.panel.from(from)}</span> : null}
          {!open && !waiting ? (
            <span className="tabular-nums">{fmtDuration(end - call.startMs, call.exactEnd)}</span>
          ) : null}
          {open ? (
            <span
              aria-hidden
              className="absolute inset-y-0 right-0 w-1 animate-pulse bg-info motion-reduce:animate-none motion-reduce:bg-info/60"
            />
          ) : null}
        </TooltipTrigger>
        <TooltipContent>{name}</TooltipContent>
      </Tooltip>
    </>
  )
}

/** Every other lane is a faint band across its name and its track, so a bar always reads against its lane. */
const STRIPE = 'bg-muted/40'

/**
 * How strongly a bar's series fill shows over the card. Pastels at a third vanish on white, so light mode runs them
 * twice as strong; dark cards need less. `--bar-dim` is a bar off the critical path.
 */
const BAR_TINT = '[--bar:70%] [--bar-dim:30%] dark:[--bar:34%] dark:[--bar-dim:16%]'

const tint = (fill: string, pct: string) => `color-mix(in oklab, ${fill} ${pct}, transparent)`
