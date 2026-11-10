/**
 * The coding harness preview (2026-11-06; design: the "Harness Preview Variations" canvas, C with B's timeline folded
 * in). While no coding harness is connected, the summary's "No coding harnesses connected" tile (and the first run's
 * Coding harnesses cell) carries Watch the preview, which plays Home from a developer's first command to a month in, on
 * sample numbers (`preview.ts`), in a dialog:
 * the command, the first sessions, then Home's own tiles growing (Week 1: harness spend beside agent runs; Month 1:
 * models, subagents and tools as tiles of their own). The timeline's stops jump; previous, pause and next step; the end
 * hands over the commands for the team and Connect yours. Under reduced motion nothing grows or plays: each step shows
 * whole and only the controls move on.
 */
import { Link } from '@tanstack/react-router'
import {
  ChevronLeft,
  ChevronRight,
  Cpu,
  GitFork,
  Pause,
  Play,
  RotateCcw,
  SquareTerminal,
  Workflow,
  Wrench,
  X,
  type LucideIcon,
} from 'lucide-react'
import { useReducedMotion } from 'motion/react'
import { useEffect, useState, type CSSProperties, type ReactNode } from 'react'
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { KpiTile } from '@nasiko/ui/composites/kpi-tile'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import { Card as Tile } from '@nasiko/ui/primitives/card'
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@nasiko/ui/primitives/dialog'
import { seriesAt } from '@nasiko/ui/lib/chart'
import { fmtInt, fmtMoney, fmtPct } from '@nasiko/ui/lib/format'
import { useCopy } from '@nasiko/ui/lib/useCopy'
import { cn } from '@nasiko/ui/lib/utils'
import { connectWithoutLogin } from '@nasiko/features/cli/commands'
import { copy as cliCopy } from '@nasiko/features/cli/copy'
import { HARNESSES } from '@nasiko/features/harnesses/constants'
import { copy } from '../copy'
import {
  PREVIEW_HARNESSES,
  PREVIEW_MODELS,
  PREVIEW_MS,
  PREVIEW_SESSIONS,
  PREVIEW_TOOLS,
  previewFrame,
  revealed,
  stepStart,
  type PreviewFrame,
} from '../preview'
import { PREVIEW_TICK_MS } from '../tuning'
import { TOUCH } from './Card'

const p = copy.harnessPreview
const NAMES = PREVIEW_HARNESSES.map((id) => HARNESSES.find((h) => h.id === id)?.name ?? id)
/** A harness's colour, as the Harnesses page draws it. */
const swatch = (i: number) => ({ '--bar': seriesAt(i).fill }) as CSSProperties
const TILE = 'min-w-0 gap-1.5 p-4'
const ROW = 'grid items-center gap-2 text-xs'

/** The play button, in the "no coding harnesses connected" tile (user 2026-11-06: no section of its own), and the
 *  dialog it opens. */
export function WatchPreview({ className }: { className?: string }) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <Button
        size="sm"
        className={cn('gap-2 pl-1.5', TOUCH, className)}
        onClick={() => setOpen(true)}
      >
        <span
          aria-hidden
          className="flex size-5 items-center justify-center rounded-full bg-primary-foreground text-primary-text"
        >
          <Play className="size-2.5 fill-current" />
        </span>
        {p.watch}
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        {/* Unmounted when closed, so every open starts from Today. */}
        <DialogContent
          showCloseButton={false}
          className="flex h-[min(46rem,calc(100dvh-2rem))] flex-col gap-4 p-5 sm:max-w-5xl sm:p-7"
        >
          <Player />
        </DialogContent>
      </Dialog>
    </>
  )
}

function Player() {
  const reduced = useReducedMotion() ?? false
  const [elapsed, setElapsed] = useState(() => stepStart(0, reduced))
  const [playing, setPlaying] = useState(!reduced)
  useEffect(() => {
    if (!playing) return
    const t = setInterval(
      () => setElapsed((e) => Math.min(PREVIEW_MS, e + PREVIEW_TICK_MS)),
      PREVIEW_TICK_MS,
    )
    return () => clearInterval(t)
  }, [playing])
  const f = previewFrame(elapsed)
  if (playing && f.done) setPlaying(false)
  const go = (step: number) => {
    const at = stepStart(step, reduced)
    setElapsed(at)
    setPlaying(!reduced && at < PREVIEW_MS)
  }
  const stop = p.stops[f.step] ?? p.stops[0]
  return (
    <>
      <div className="flex items-start gap-3">
        <nav aria-label={p.timeline} className="grid flex-1 grid-cols-4 gap-1.5">
          {p.stops.map((s, i) => (
            <Button
              key={s.label}
              variant="ghost"
              aria-label={p.jump(s.label)}
              aria-current={i === f.step ? 'step' : undefined}
              onClick={() => go(i)}
              className={cn(
                'h-auto flex-col items-stretch gap-2 rounded-sm px-0 py-1 hover:bg-transparent',
                TOUCH,
              )}
            >
              <span aria-hidden className="block h-1 overflow-hidden rounded-full bg-muted">
                <span
                  className="block h-full bg-foreground"
                  style={{
                    width: `${f.done || i < f.step ? 100 : i === f.step ? f.progress * 100 : 0}%`,
                  }}
                />
              </span>
              <span
                className={cn(
                  'text-left text-xs',
                  i === f.step
                    ? 'font-semibold text-foreground'
                    : 'font-normal text-muted-foreground',
                )}
              >
                {s.label}
              </span>
            </Button>
          ))}
        </nav>
        <Badge variant="warning" className="mt-0.5 uppercase">
          {p.sample}
        </Badge>
        <DialogClose render={<Button variant="ghost" size="icon-sm" aria-label={p.close} />}>
          <X aria-hidden />
        </DialogClose>
      </div>

      <div className="flex items-end gap-4">
        <DialogHeader className="flex-1 gap-1 text-left sm:text-left">
          <DialogTitle className="text-xl">{stop.title}</DialogTitle>
          <DialogDescription className="text-pretty">{stop.body}</DialogDescription>
        </DialogHeader>
        {f.step >= 2 ? (
          <Badge variant="outline" className="shrink-0 tabular-nums">
            {p.day(f.day)}
          </Badge>
        ) : null}
      </div>

      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto rounded-xl bg-muted/50 p-3">
        {f.step === 0 ? (
          <Terminal frame={f} />
        ) : f.step === 1 ? (
          <Sessions frame={f} />
        ) : (
          <Tiles frame={f} />
        )}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <Button
          variant="outline"
          size="icon"
          className={cn('rounded-full', TOUCH)}
          aria-label={p.prev}
          onClick={() => go(f.step - 1)}
        >
          <ChevronLeft aria-hidden />
        </Button>
        {f.done ? (
          <Button
            variant="outline"
            size="icon"
            className={cn('rounded-full', TOUCH)}
            aria-label={p.replay}
            onClick={() => go(0)}
          >
            <RotateCcw aria-hidden />
          </Button>
        ) : reduced ? null : (
          <Button
            variant="outline"
            size="icon"
            className={cn('rounded-full', TOUCH)}
            aria-label={playing ? p.pause : p.play}
            onClick={() => setPlaying(!playing)}
          >
            {playing ? <Pause aria-hidden /> : <Play aria-hidden />}
          </Button>
        )}
        <Button
          variant="outline"
          size="icon"
          className={cn('rounded-full', TOUCH)}
          aria-label={p.next}
          onClick={() => go(f.step + 1)}
        >
          <ChevronRight aria-hidden />
        </Button>
        {f.done ? <Ending /> : null}
      </div>
    </>
  )
}

/** Today: the one command, its answers appearing in turn. Signing in connects and imports (`nasiko auth login`). */
function Terminal({ frame }: { frame: PreviewFrame }) {
  const sso = !!useSlots().loginMethods
  const lines = revealed(frame.progress, p.terminal.length + 1) - 1
  return (
    <Tile className="flex-1 gap-0 p-5 font-mono text-sm leading-8">
      <p>
        <span aria-hidden className="text-muted-foreground select-none">
          ${' '}
        </span>
        {sso ? 'nasiko auth login --sso' : 'nasiko auth login'}
      </p>
      {p.terminal.slice(0, lines).map((line) => (
        <p key={line} className="text-success">
          <span aria-hidden>✓ </span>
          {line}
        </p>
      ))}
    </Tile>
  )
}

/** Day 1: the first sessions, one after another. */
function Sessions({ frame }: { frame: PreviewFrame }) {
  const shown = revealed(frame.progress, PREVIEW_SESSIONS.length)
  return (
    <Tile className="flex-1 gap-0 py-1.5">
      <div className="flex justify-between px-4 py-2.5 text-xs text-muted-foreground">
        <span>{p.sessions}</span>
        <span>{p.sessionsMeta}</span>
      </div>
      <ul>
        {PREVIEW_SESSIONS.map(([h, title, turns, cost], i) => (
          <li
            key={title}
            className={cn(
              'grid grid-cols-[7.5rem_minmax(0,1fr)_5rem_4.5rem] items-center gap-3 border-t px-4 py-2.5 text-sm transition-[opacity,translate] duration-300 ease-out motion-reduce:transition-none',
              i < shown ? 'translate-y-0 opacity-100' : 'translate-y-2 opacity-0',
            )}
          >
            <span className="flex items-center gap-2" style={swatch(h)}>
              <span aria-hidden className="size-2 rounded-xs bg-(--bar)" />
              {NAMES[h]}
            </span>
            <span className="truncate">{title}</span>
            <span className="text-right text-muted-foreground tabular-nums">{p.turns(turns)}</span>
            <span className="text-right tabular-nums">{fmtMoney(cost)}</span>
          </li>
        ))}
      </ul>
    </Tile>
  )
}

function Label({ Icon, children }: { Icon: LucideIcon; children: ReactNode }) {
  return (
    <span className="inline-flex items-center gap-1.5 text-sm text-muted-foreground">
      <Icon className="size-4" aria-hidden />
      {children}
    </span>
  )
}

/** A name, a bar and a share. */
function ShareRow({
  name,
  width,
  share,
  series,
}: {
  name: string
  width: number
  share: number
  series: number
}) {
  return (
    <div className={cn(ROW, 'grid-cols-[6.5rem_minmax(0,1fr)_2.75rem]')}>
      <span className="truncate">{name}</span>
      <span aria-hidden className="h-1.5 overflow-hidden rounded-full bg-muted">
        <span
          className="block h-full bg-(--bar)"
          style={{ ...swatch(series), width: `${Math.min(100, width)}%` }}
        />
      </span>
      <span className="text-right text-muted-foreground tabular-nums">{fmtPct(share)}</span>
    </div>
  )
}

/** Week 1 and Month 1: Home's own tiles, growing. */
function Tiles({ frame: f }: { frame: PreviewFrame }) {
  const total = f.cost.reduce((a, b) => a + b, 0)
  const on = (v: number) => v >= 0.5
  const runsBar = seriesAt(1)
  return (
    <div className="flex flex-1 flex-col gap-3">
      <div className="grid flex-1 gap-3 sm:grid-cols-2">
        <KpiTile
          className={TILE}
          label={<Label Icon={SquareTerminal}>{p.harnesses}</Label>}
          aside={
            <span className="text-xs text-muted-foreground tabular-nums">
              {p.developers(fmtInt(f.devs))}
            </span>
          }
          value={
            <>
              {fmtMoney(total)}{' '}
              <span className="text-sm font-normal text-muted-foreground">{p.est}</span>
            </>
          }
        >
          <p className="text-xs text-muted-foreground">{p.connected(f.cost.filter(on).length)}</p>
          <span
            aria-hidden
            className="mt-auto flex h-2 gap-0.5 overflow-hidden rounded-full bg-muted"
          >
            {f.cost.map((c, i) => (
              <span
                key={PREVIEW_HARNESSES[i]}
                className="h-full bg-(--bar)"
                style={{ ...swatch(i), width: `${total > 0 ? (c / total) * 100 : 0}%` }}
              />
            ))}
          </span>
          <ul className="mt-1 flex flex-wrap gap-x-4 gap-y-1 text-xs">
            {f.cost.map((c, i) => (
              <li
                key={PREVIEW_HARNESSES[i]}
                className={cn('flex items-center gap-1.5', !on(c) && 'opacity-50')}
                style={swatch(i)}
              >
                <span aria-hidden className="size-2 rounded-xs bg-(--bar)" />
                {NAMES[i]}
                <span className="text-muted-foreground tabular-nums">
                  {on(c) ? fmtMoney(c) : p.notYet}
                </span>
              </li>
            ))}
          </ul>
        </KpiTile>
        <KpiTile
          className={TILE}
          label={<Label Icon={Workflow}>{p.runs}</Label>}
          value={fmtInt(f.runs)}
        >
          <p className="text-xs text-muted-foreground tabular-nums">
            {p.runsLine(fmtInt(f.runsPerDay), fmtInt(f.runs * 1.2))}
          </p>
          <span aria-hidden className="mt-auto flex h-20 items-end gap-1.5">
            {f.bars.map((h, i) => (
              <span
                // eslint-disable-next-line @eslint-react/no-array-index-key -- a fixed set of decorative bars that never reorders
                key={i}
                className="flex-1 bg-(--bar) [border-top:2px_solid_var(--edge)]"
                style={
                  {
                    height: `${h * 100}%`,
                    '--bar': runsBar.fill,
                    '--edge': runsBar.edge,
                  } as CSSProperties
                }
              />
            ))}
          </span>
        </KpiTile>
      </div>
      {f.step === 3 ? (
        <div className="grid gap-3 sm:grid-cols-3">
          <KpiTile
            className={TILE}
            label={<Label Icon={Cpu}>{p.models}</Label>}
            value={fmtPct(f.models[0])}
          >
            <p className="text-xs text-muted-foreground">{p.topModel(PREVIEW_MODELS[0])}</p>
            <div className="mt-auto flex flex-col gap-1.5 pt-2">
              {PREVIEW_MODELS.map((name, i) => (
                <ShareRow
                  key={name}
                  name={name}
                  share={f.models[i] ?? 0}
                  width={f.models[i] ?? 0}
                  series={0}
                />
              ))}
            </div>
          </KpiTile>
          <KpiTile
            className={TILE}
            label={<Label Icon={GitFork}>{p.subagents}</Label>}
            value={fmtInt(f.subagents.reduce((a, b) => a + b, 0))}
          >
            <p className="text-xs text-muted-foreground">{p.subagentsLine}</p>
            <ul className="mt-auto flex flex-col gap-1 pt-2 text-xs">
              {f.subagents.map((runs, i) => (
                <li
                  key={PREVIEW_HARNESSES[i]}
                  className={cn('flex justify-between gap-3', !on(runs) && 'opacity-50')}
                >
                  <span>{NAMES[i]}</span>
                  <span className="text-muted-foreground tabular-nums">
                    {on(runs) ? fmtInt(runs) : p.notYet}
                  </span>
                </li>
              ))}
            </ul>
          </KpiTile>
          <KpiTile
            className={TILE}
            label={<Label Icon={Wrench}>{p.tools}</Label>}
            value={fmtInt(f.calls)}
          >
            <p className="text-xs text-muted-foreground">{p.toolsLine}</p>
            <div className="mt-auto flex flex-col gap-1.5 pt-2">
              {PREVIEW_TOOLS.map((name, i) => (
                <ShareRow
                  key={name}
                  name={name}
                  share={f.tools[i]?.share ?? 0}
                  width={f.tools[i]?.width ?? 0}
                  series={2}
                />
              ))}
            </div>
          </KpiTile>
        </div>
      ) : null}
    </div>
  )
}

/** The end: every developer runs the commands on their own machine, so hand them over, then connect this one. */
function Ending() {
  const sso = !!useSlots().loginMethods
  const commands = [
    cliCopy.install.unixCommand,
    connectWithoutLogin(),
    sso ? 'nasiko auth login --sso' : 'nasiko auth login',
  ].join('\n')
  const [state, run] = useCopy(commands)
  return (
    <div className="ml-auto flex flex-wrap items-center justify-end gap-2">
      <span className="mr-2 text-sm text-muted-foreground">{p.ending}</span>
      <Button variant="outline" className={TOUCH} onClick={() => void run()}>
        {state === 'copied' ? p.copied : p.copyCommands}
      </Button>
      <Link to="/harnesses" search={{} as never} {...asButton({ className: TOUCH })}>
        {p.connectYours}
      </Link>
      {/* The clipboard can be blocked (plain HTTP): the commands stay selectable. */}
      {state === 'failed' ? (
        <pre className="w-full overflow-x-auto rounded-md border bg-muted/50 p-2 font-mono text-xs select-all">
          {commands}
        </pre>
      ) : null}
    </div>
  )
}
