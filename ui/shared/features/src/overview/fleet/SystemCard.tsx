/**
 * The System card, in the canvas's top left in both views (Ans, 2026-10-17: the system's health where the fleet
 * shield used to be, in the page's own card style): folded by default to its counts (every part, what needs
 * attention, what is healthy), it opens to every part of the workspace as one list (system.ts), with a search, three
 * filters (All, Needs attention, Healthy) and a fold per kind. A row picks the part, as a click on it does. It
 * replaced the village's List view (2026-10-17).
 *
 * It reads as part of the board (Ans, 2026-10-17): one font (the app's sans; no mono, no capitals), the board's tones
 * for its dots (`fb-tones` scopes the board's palette here, `fb-dot` draws them), and the page's own outline toggle for
 * the filters, unstyled.
 */
import { ChevronDown, ChevronUp } from 'lucide-react'
import { useMemo, useState } from 'react'
import { SearchInput } from '@nasiko/ui/composites/search-input'
import { Button } from '@nasiko/ui/primitives/button'
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@nasiko/ui/primitives/collapsible'
import { ToggleGroup, ToggleGroupItem } from '@nasiko/ui/primitives/toggle-group'
import { cn } from '@nasiko/ui/lib/utils'
import type { EstateView } from '../estate/useEstateView'
import './board.css'
import { CARD } from '../village/styles'
import { boardCopy } from './copy'
import type { Board, Tone } from './model'
import {
  filterSystem,
  systemCounts,
  systemSections,
  worst,
  type SystemFilter,
  type SystemGroup,
} from './system'

const copy = boardCopy.system

/** A part's state in the board's tones (board.css `fb-dot`). */
function Dot({ tone }: { tone: Tone }) {
  return <span aria-hidden className={cn('fb-dot', `t-${tone}`)} />
}

export function SystemCard({
  v,
  board,
  sel,
  onPick,
  open,
  onOpen,
  className,
}: {
  v: EstateView
  board: Board
  sel: string | undefined
  onPick(sel: string): void
  /** Open to the list, or folded to its counts. */
  open: boolean
  onOpen(open: boolean): void
  className?: string
}) {
  const sections = useMemo(() => systemSections(v, board), [v, board])
  const counts = systemCounts(sections)
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<SystemFilter>('all')
  const [closed, setClosed] = useState<ReadonlySet<string>>(() => new Set())
  const shown = filterSystem(sections, query, filter)
  const fold = (key: string, open: boolean) =>
    setClosed((c) => {
      const next = new Set(c)
      if (open) next.delete(key)
      else next.add(key)
      return next
    })
  const filters: [SystemFilter, string, Tone | null][] = [
    ['all', copy.all, null],
    ['attention', copy.attention, 'bad'],
    ['healthy', copy.healthy, 'ok'],
  ]

  if (!open)
    return (
      <Button
        data-vp-ignore=""
        variant="ghost"
        className={cn(CARD, 'fb-tones h-9 gap-2.5 bg-card px-3 text-xs hover:bg-card', className)}
        aria-label={copy.openLabel}
        aria-expanded={false}
        onClick={() => onOpen(true)}
      >
        <span className="font-semibold">{copy.title}</span>
        <span className="inline-flex items-center gap-1.5 text-muted-foreground tabular-nums">
          <Dot tone="bad" />
          {counts.attention}
        </span>
        <span className="inline-flex items-center gap-1.5 text-muted-foreground tabular-nums">
          <Dot tone="ok" />
          {counts.healthy}
        </span>
        <ChevronDown aria-hidden className="text-muted-foreground" />
      </Button>
    )
  return (
    <section
      data-vp-ignore=""
      aria-label={copy.title}
      className={cn(CARD, 'fb-tones flex w-[300px] flex-col bg-card', className)}
    >
      <div className="flex items-center gap-2 px-3.5 pt-3">
        <h2 className="text-sm font-semibold">{copy.title}</h2>
        <span className="ml-auto text-xs text-muted-foreground tabular-nums">
          {copy.parts(counts.all)}
        </span>
        <Button
          variant="ghost"
          size="icon-xs"
          className="text-muted-foreground"
          aria-label={copy.hide}
          aria-expanded
          onClick={() => onOpen(false)}
        >
          <ChevronUp aria-hidden />
        </Button>
      </div>
      <div className="flex flex-col gap-2 px-3.5 pt-2.5 pb-2">
        <SearchInput
          className="h-8 max-w-none"
          placeholder={copy.search}
          aria-label={copy.search}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <ToggleGroup
          variant="outline"
          size="sm"
          value={[filter]}
          onValueChange={(vs) => {
            const x = vs[0] as string
            return x && setFilter(x as SystemFilter)
          }}
          aria-label={copy.show}
          className="w-full"
        >
          {filters.map(([key, label, tone]) => (
            <ToggleGroupItem key={key} value={key} className="gap-1.5 px-2 text-xs">
              {tone ? <Dot tone={tone} /> : null}
              {label}
              <span className="tabular-nums">{counts[key]}</span>
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-2 pb-2">
        {shown.length ? (
          shown.map((s) => (
            <div key={s.key} className="mt-2 first:mt-1">
              <h3 className="flex items-baseline gap-1.5 px-1.5 py-1 text-xs font-semibold">
                {s.label}
                <span className="font-normal text-muted-foreground tabular-nums">
                  {s.groups.reduce((n, g) => n + g.rows.length, 0)}
                </span>
              </h3>
              {s.groups.map((g) => (
                <Group
                  key={g.key}
                  group={g}
                  open={!!query || !closed.has(g.key)}
                  onOpen={(o) => fold(g.key, o)}
                  sel={sel}
                  onPick={onPick}
                />
              ))}
            </div>
          ))
        ) : (
          <p className="px-1.5 py-6 text-center text-sm text-muted-foreground">{copy.empty}</p>
        )}
      </div>
    </section>
  )
}

function Group({
  group,
  open,
  onOpen,
  sel,
  onPick,
}: {
  group: SystemGroup
  open: boolean
  onOpen(open: boolean): void
  sel: string | undefined
  onPick(sel: string): void
}) {
  return (
    <Collapsible open={open} onOpenChange={onOpen}>
      <CollapsibleTrigger
        render={
          <Button
            variant="ghost"
            size="sm"
            className="h-8 w-full justify-start gap-1.5 px-1.5 font-normal text-muted-foreground"
          />
        }
      >
        <ChevronDown
          aria-hidden
          className={cn('size-3.5 transition-transform', !open && '-rotate-90')}
        />
        {group.label}
        <Dot tone={worst(group.rows)} />
        <span className="ml-auto text-xs tabular-nums">{group.rows.length}</span>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <ul className="flex flex-col">
          {group.rows.map((r) => (
            <li key={`${group.key}:${r.sel}:${r.name}`}>
              <Button
                variant="ghost"
                size="sm"
                aria-pressed={sel === r.sel}
                className="h-8 w-full justify-start gap-2.5 pr-1.5 pl-3 font-normal aria-pressed:bg-accent"
                onClick={() => onPick(r.sel)}
              >
                <Dot tone={r.tone} />
                <span className="min-w-0 flex-1 truncate text-left">{r.name}</span>
                <span className="text-xs text-muted-foreground tabular-nums">{r.value}</span>
              </Button>
            </li>
          ))}
        </ul>
      </CollapsibleContent>
    </Collapsible>
  )
}
