/**
 * The problem card, the same everywhere a problem shows (the insight panel, a day tile, a flow, a card summary): its
 * kind and tier, the title with its number, why it matters, where the metric is against its target, ten days of
 * trend, who is on it (Assign, Reassign, Unassign, I'll take it), and one primary action into the owning page, then
 * the cause chain, the evidence, Snooze and Mark handled. Snooze and Mark handled leave the lists at once and offer
 * Undo; handled reopens tomorrow if the numbers are still behind (the server's rule, OV-2). Without OV-2 the owner
 * row says assigning needs a newer server, and Snooze and Mark handled are left out.
 */
import { Check, ChevronDown, Link2, UserPlus } from 'lucide-react'
import { useState } from 'react'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button } from '@nasiko/ui/primitives/button'
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
} from '@nasiko/ui/primitives/command'
import { Popover, PopoverContent, PopoverTrigger } from '@nasiko/ui/primitives/popover'
import { fmtShortDay } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import { estateCopy as copy } from '../estate/copy'
import type { Metric, Person, Problem, ProblemPatch } from '../estate/types'
import type { EstateView } from '../estate/useEstateView'
import { STRONG_BG, STRONG_BORDER, STRONG_STROKE, STRONG_TEXT } from '../map/tones'
import { TargetLink } from './targets'

export interface ProblemActions {
  /** OV-2 is on this server. */
  enabled: boolean
  me: string
  patch(id: string, patch: ProblemPatch, undo?: ProblemPatch, message?: string): void
  showChain(id: string): void
}

export function ProblemCard({
  problem: p,
  v,
  actions,
  compact,
}: {
  problem: Problem
  v: EstateView
  actions: ProblemActions
  /** In a list of several: no trend chart, actions trimmed to the primary one and the chain. */
  compact?: boolean
}) {
  const [open, setOpen] = useState(false)
  const owner = p.owner_id ? (v.estate.people.find((x) => x.id === p.owner_id) ?? null) : null
  const me = owner?.id === actions.me
  const hasMetric = p.metric.max > p.metric.min
  return (
    <article
      className={cn('rounded-lg border border-l-[3px] bg-card px-3.5 py-3', STRONG_BORDER[p.kind])}
      aria-labelledby={`problem-${p.id}`}
      data-testid="problem-card"
    >
      <div className="mb-1.5 flex flex-wrap items-center gap-1.5">
        <span
          className={cn(
            'rounded px-1.5 text-2xs leading-4 font-semibold text-card',
            STRONG_BG[p.kind],
          )}
        >
          {copy.kind[p.kind]}
        </span>
        <Badge variant="outline" className="h-4 px-1.5 text-2xs font-normal text-muted-foreground">
          {copy.tier[p.tier]}
        </Badge>
        {p.also ? (
          <Badge
            variant="outline"
            className="h-4 px-1.5 text-2xs font-normal text-muted-foreground"
          >
            {copy.card.also(copy.kind[p.also])}
          </Badge>
        ) : null}
        <span className="ml-auto text-2xs text-muted-foreground tabular-nums">
          {copy.card.since(fmtShortDay(p.since))}
        </span>
      </div>
      <h4 id={`problem-${p.id}`} className="text-sm leading-5 font-semibold text-pretty">
        {p.title}
      </h4>
      <p className="mt-0.5 text-xs leading-4 text-muted-foreground">{p.why}</p>
      {hasMetric ? (
        <div className="mt-2.5 flex items-end gap-3">
          <Distance m={p.metric} kind={p.kind} />
          {!compact && p.trend.length > 1 ? <Spark values={p.trend} kind={p.kind} /> : null}
        </div>
      ) : null}
      <Owner p={p} owner={owner} me={me} v={v} actions={actions} />
      <div className="mt-2.5 flex flex-wrap items-center gap-1">
        <TargetLink target={p.action.target} estate={v.estate} className="h-7 px-2.5 text-xs">
          {p.action.label}
        </TargetLink>
        {p.chain_id ? (
          <Button
            variant="outline"
            size="sm"
            onClick={() => actions.showChain(p.chain_id ?? '')}
            className="h-7 px-2.5 text-xs pointer-coarse:min-h-11"
          >
            <Link2 aria-hidden /> {copy.card.showChain}
          </Button>
        ) : null}
        <Button
          variant="ghost"
          size="sm"
          className="h-7 px-2 text-xs text-muted-foreground pointer-coarse:min-h-11"
          aria-expanded={open}
          onClick={() => setOpen((x) => !x)}
        >
          {open ? copy.card.hideEvidence : copy.card.evidence}
        </Button>
        {actions.enabled ? (
          <>
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-2 text-xs text-muted-foreground pointer-coarse:min-h-11"
              aria-label={copy.card.snoozeLabel(p.title)}
              onClick={() =>
                actions.patch(
                  p.id,
                  { snoozed_until: tomorrow() },
                  { snoozed_until: null },
                  copy.card.toast.snoozed,
                )
              }
            >
              {copy.card.snooze}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-2 text-xs text-muted-foreground pointer-coarse:min-h-11"
              aria-label={copy.card.handledLabel(p.title)}
              onClick={() =>
                actions.patch(p.id, { handled: true }, { handled: false }, copy.card.toast.handled)
              }
            >
              {copy.card.handled}
            </Button>
          </>
        ) : null}
      </div>
      {open ? (
        <div className="mt-2 rounded-md bg-muted/60 px-3 py-2 text-xs leading-4">
          <p className="mb-1 text-2xs font-medium text-muted-foreground">{copy.card.whatWeSaw}</p>
          <ul className="mb-2 list-disc space-y-0.5 pl-4">
            {p.evidence.map((x) => (
              <li key={x}>{x}</li>
            ))}
          </ul>
          <p className="mb-1 text-2xs font-medium text-muted-foreground">{copy.card.fixedWhen}</p>
          <p>{p.fixed_when}</p>
          <p className="mt-1.5 text-muted-foreground">{copy.card.confidence[p.confidence]}</p>
        </div>
      ) : null}
    </article>
  )
}

const tomorrow = () => {
  const d = new Date()
  d.setDate(d.getDate() + 1)
  d.setHours(9, 0, 0, 0)
  return d.toISOString()
}

const initials = (name: string) =>
  name
    .split(/[\s.]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0]?.toUpperCase())
    .join('')

function Owner({
  p,
  owner,
  me,
  v,
  actions,
}: {
  p: Problem
  owner: Person | null
  me: boolean
  v: EstateView
  actions: ProblemActions
}) {
  const [open, setOpen] = useState(false)
  const people = [...v.estate.people].sort(
    (a, b) =>
      Number(b.id === actions.me) - Number(a.id === actions.me) || a.name.localeCompare(b.name),
  )
  const assign = (to: string | null) => {
    setOpen(false)
    const name = to ? (v.estate.people.find((x) => x.id === to)?.name ?? '') : ''
    actions.patch(
      p.id,
      { owner_id: to },
      { owner_id: p.owner_id },
      to ? copy.card.toast.assigned(name, to === actions.me) : copy.card.toast.unassigned,
    )
  }
  return (
    <div className="mt-2.5 flex flex-wrap items-center gap-2 rounded-md bg-muted/60 px-2 py-1.5 text-xs">
      <span
        aria-hidden
        className={cn(
          'grid size-5.5 shrink-0 place-items-center rounded-full text-3xs font-semibold',
          owner ? 'bg-foreground text-background' : 'bg-border text-muted-foreground',
        )}
      >
        {owner ? initials(owner.name) : '?'}
      </span>
      <span className="min-w-0 flex-1 basis-28">
        {owner ? (
          <span className="font-medium">
            {me ? copy.card.whoMe : copy.card.whoOther(owner.name)}
          </span>
        ) : (
          <span className="text-muted-foreground">{copy.card.whoNone}</span>
        )}
      </span>
      {actions.enabled ? (
        <span className="ml-auto flex shrink-0 items-center gap-1">
          {!me ? (
            <Button
              variant="outline"
              size="sm"
              className="h-6 px-2 text-xs pointer-coarse:min-h-11"
              onClick={() => assign(actions.me)}
            >
              {copy.card.take}
            </Button>
          ) : null}
          <Popover open={open} onOpenChange={setOpen}>
            <PopoverTrigger
              render={
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-6 px-1.5 text-xs pointer-coarse:min-h-11"
                  aria-label={copy.card.assignTo}
                />
              }
            >
              <UserPlus aria-hidden className="size-3.5" />
              {owner ? copy.card.reassign : copy.card.assign}
              <ChevronDown aria-hidden className="size-3" />
            </PopoverTrigger>
            <PopoverContent className="w-64 p-0" align="end">
              <Command>
                <CommandInput placeholder={copy.card.searchPeople} />
                <CommandList>
                  <CommandEmpty>{copy.card.noPeople}</CommandEmpty>
                  <CommandGroup>
                    {people.map((x) => (
                      <CommandItem
                        key={x.id}
                        value={`${x.name} ${x.id}`}
                        onSelect={() => assign(x.id)}
                      >
                        <span
                          aria-hidden
                          className="grid size-5 place-items-center rounded-full bg-muted text-3xs font-semibold"
                        >
                          {initials(x.name)}
                        </span>
                        <span className="truncate">
                          {x.id === actions.me ? `${x.name} (${copy.card.me})` : x.name}
                        </span>
                        {x.id === p.owner_id ? (
                          <Check aria-hidden className="ml-auto size-3.5" />
                        ) : null}
                      </CommandItem>
                    ))}
                  </CommandGroup>
                  {owner ? (
                    <>
                      <CommandSeparator />
                      <CommandGroup>
                        <CommandItem value="__unassign" onSelect={() => assign(null)}>
                          {copy.card.unassign}
                        </CommandItem>
                      </CommandGroup>
                    </>
                  ) : null}
                </CommandList>
              </Command>
            </PopoverContent>
          </Popover>
        </span>
      ) : (
        <span className="text-2xs text-muted-foreground">{copy.card.readOnly}</span>
      )}
    </div>
  )
}

const fmt = (v: number) => (Number.isInteger(v) ? String(v) : v.toFixed(1))

/** Where the metric is, where its target is, and the gap between them in the problem's colour. */
export function Distance({ m, kind }: { m: Metric; kind: Problem['kind'] }) {
  const at = (x: number) => ((Math.max(m.min, Math.min(m.max, x)) - m.min) / (m.max - m.min)) * 100
  const cur = at(m.current)
  const target = at(m.target)
  const gapFrom = Math.min(cur, target)
  const gapTo = Math.max(cur, target)
  const behind = m.direction === 'lower' ? m.current > m.target : m.current < m.target
  const [now, vs] = copy.card.vsTarget(`${fmt(m.current)}${m.unit}`, `${fmt(m.target)}${m.unit}`)
  return (
    <div className="min-w-0 flex-1">
      <div className="mb-1 flex justify-between gap-2 text-2xs text-muted-foreground">
        <span className="truncate">{m.label}</span>
        <span className="shrink-0 tabular-nums">
          <b className="font-semibold text-foreground">{now}</b> {vs}
        </span>
      </div>
      <div
        className="relative h-2 rounded-full bg-muted"
        role="img"
        aria-label={`${m.label}: ${now} ${vs}`}
      >
        <div
          className="absolute inset-y-0 left-0 rounded-full bg-muted-foreground/25"
          style={{ width: `${gapFrom}%` }}
        />
        {behind ? (
          <div
            className={cn('absolute inset-y-0', STRONG_BG[kind])}
            style={{ left: `${gapFrom}%`, width: `${gapTo - gapFrom}%` }}
          />
        ) : null}
        <div
          className="absolute -top-1 -bottom-1 w-0.5 rounded bg-foreground"
          style={{ left: `calc(${target}% - 1px)` }}
        />
        <div
          className={cn(
            'absolute top-1/2 size-2.5 -translate-x-1/2 -translate-y-1/2 rounded-full ring-2 ring-card',
            STRONG_BG[kind],
          )}
          style={{ left: `${cur}%` }}
        />
      </div>
    </div>
  )
}

/** Ten days of the metric, in the problem's colour. Decorative: the distance bar carries the numbers. */
export function Spark({
  values,
  kind,
  width = 64,
}: {
  values: readonly number[]
  kind: Problem['kind']
  width?: number
}) {
  const h = 24
  const mx = Math.max(...values)
  const mn = Math.min(...values)
  const r = mx - mn || 1
  const pts = values
    .map(
      (x, i) =>
        `${((i * (width - 4)) / (values.length - 1) + 2).toFixed(1)},${(h - 3 - ((x - mn) / r) * (h - 6)).toFixed(1)}`,
    )
    .join(' ')
  return (
    <svg
      aria-hidden
      viewBox={`0 0 ${width} ${h}`}
      width={width}
      height={h}
      className={cn('shrink-0', STRONG_TEXT[kind])}
    >
      <polyline
        points={pts}
        fill="none"
        className={STRONG_STROKE[kind]}
        strokeWidth={1.6}
        strokeLinejoin="round"
      />
    </svg>
  )
}
