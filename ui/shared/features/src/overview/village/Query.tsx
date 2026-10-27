/**
 * The canvas's find box: any part of the estate (a team, a person, an agent, a workflow, a model, a tool) or one of
 * the scope's open problems, and open it. Picking a result is a selection like a click on the canvas: the view eases to
 * it and the drawer explains it. "Look in" narrows what it searches (All by default). On the Map it is the Go to box in
 * the top left (`GoTo`), its results opening below it; the village's bottom centre has the chat box instead (Ask.tsx).
 */
import { Search } from 'lucide-react'
import { useState } from 'react'
import { Button } from '@nasiko/ui/primitives/button'
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@nasiko/ui/primitives/command'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@nasiko/ui/primitives/select'
import { cn } from '@nasiko/ui/lib/utils'
import { estateCopy } from '../estate/copy'
import type { NodeKind } from '../estate/types'
import type { EstateView } from '../estate/useEstateView'
import { villageCopy as copy } from './copy'
import { PoseMark } from './Hud'
import { CARD } from './styles'

const SCOPES = ['all', 'problems', 'agents', 'teams', 'models', 'tools'] as const
type Scope = (typeof SCOPES)[number]

/** The part kinds each scope searches; `problems` searches only problems, `all` everything. */
const KINDS: Record<Exclude<Scope, 'all' | 'problems'>, readonly NodeKind[]> = {
  agents: ['agent', 'workflow', 'harness'],
  teams: ['group', 'person'],
  models: ['provider', 'model', 'router'],
  tools: ['tool'],
}
/** How many problems show before anything is typed. */
const PROBLEMS_SHOWN = 5
/** A result's value is its searchable text, then its id (cmdk needs unique values) after this mark. */
const ID_MARK = '\u0000'
/** Plain substring matching on the text: cmdk's fuzzy score lists too much on a canvas of short names. */
const matches = (value: string, search: string) =>
  value.slice(0, value.indexOf(ID_MARK)).toLowerCase().includes(search.trim().toLowerCase()) ? 1 : 0

export function CanvasQuery({
  v,
  onPick,
  className,
  below = false,
  scoped = true,
  focusOnOpen = false,
  onDismiss,
}: {
  v: EstateView
  /** The selection to open, as the page's raw `sel`. */
  onPick(sel: string): void
  className?: string
  /** The results open under the box (it sits at the top), not over it. */
  below?: boolean
  /** With the "Look in" menu. */
  scoped?: boolean
  /** Takes the focus as it opens (Go to opens it on a click, to be typed in at once). */
  focusOnOpen?: boolean
  /** Escape, or the focus leaving the box: Go to folds it back. */
  onDismiss?: () => void
}) {
  const [scope, setScope] = useState<Scope>('all')
  const [text, setText] = useState('')
  const [open, setOpen] = useState(false)
  const typed = text.trim().length > 0

  const kinds = scope === 'all' ? null : scope === 'problems' ? [] : KINDS[scope]
  const parts =
    typed && kinds?.length !== 0
      ? [...v.nodes.values()].filter((n) => !kinds || kinds.includes(n.kind))
      : []
  // Problems are about their first entity: opening it shows every card the part has, this one first.
  const problems =
    scope === 'all' || scope === 'problems'
      ? v.problems.filter((p) => p.entities[0]).slice(0, typed ? undefined : PROBLEMS_SHOWN)
      : []

  const pick = (sel: string) => {
    onPick(sel)
    setText('')
    setOpen(false)
  }

  return (
    <Command
      label={copy.query.label}
      loop
      filter={matches}
      className={cn('relative overflow-visible bg-transparent', className)}
      onKeyDown={(e) => {
        // Escape closes the results first; the canvas's Escape (clear the selection) only when they are shut.
        if (e.key === 'Escape' && open) {
          e.stopPropagation()
          setOpen(false)
        } else if (e.key === 'Escape' && onDismiss) {
          e.stopPropagation()
          onDismiss()
        }
      }}
    >
      {open && (typed || problems.length) ? (
        <CommandList
          className={cn(
            'absolute inset-x-0 z-10 max-h-72 rounded-xl border bg-popover shadow-md',
            below ? 'top-full mt-2' : 'bottom-full mb-2',
          )}
          // Focus stays in the box, so a click on a result isn't lost to the box's blur closing the list.
          onMouseDown={(e) => e.preventDefault()}
        >
          <CommandEmpty>{copy.query.empty}</CommandEmpty>
          {problems.length ? (
            <CommandGroup heading={copy.query.problems}>
              {problems.map((p) => (
                <CommandItem
                  key={p.id}
                  value={`${p.title}${ID_MARK}${p.id}`}
                  onSelect={() => pick(`node:${p.entities[0]}`)}
                >
                  <PoseMark pose={p.tier === 'broken' ? 'down' : 'hurt'} />
                  <span className="truncate">{p.title}</span>
                </CommandItem>
              ))}
            </CommandGroup>
          ) : null}
          {parts.length ? (
            <CommandGroup heading={copy.query.parts}>
              {parts.map((n) => (
                <CommandItem
                  key={n.id}
                  value={`${n.name} ${estateCopy.nodeKind[n.kind]}${ID_MARK}${n.id}`}
                  onSelect={() => pick(`node:${n.id}`)}
                >
                  <span className="truncate">{n.name}</span>{' '}
                  <span className="ml-auto shrink-0 text-xs text-muted-foreground">
                    {estateCopy.nodeKind[n.kind]}
                  </span>
                </CommandItem>
              ))}
            </CommandGroup>
          ) : null}
        </CommandList>
      ) : null}
      <div
        className={cn(
          CARD,
          'flex h-10 items-center gap-1 pr-1 pl-1 [&_[data-slot=command-input-wrapper]]:h-9 [&_[data-slot=command-input-wrapper]]:flex-1 [&_[data-slot=command-input-wrapper]]:border-0 [&_[data-slot=command-input-wrapper]]:px-1.5 [&_[data-slot=command-input-wrapper]>svg]:hidden',
        )}
      >
        {scoped ? (
          <Select value={scope} onValueChange={(s) => setScope(s as Scope)}>
            <SelectTrigger
              size="sm"
              aria-label={copy.query.scopeLabel}
              className="h-8 shrink-0 gap-1 border-0 bg-muted px-2.5 text-xs shadow-none dark:bg-muted"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {SCOPES.map((s) => (
                <SelectItem key={s} value={s} className="text-xs">
                  {copy.query.scopes[s]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : null}
        <Search aria-hidden className="ml-1 size-4 shrink-0 text-muted-foreground" />
        <CommandInput
          value={text}
          onValueChange={(t) => {
            setText(t)
            setOpen(true)
          }}
          onFocus={() => setOpen(true)}
          // eslint-disable-next-line jsx-a11y/no-autofocus -- Go to opens the box on a click, to be typed in at once
          autoFocus={focusOnOpen}
          onBlur={() => {
            setOpen(false)
            onDismiss?.()
          }}
          placeholder={copy.query.placeholder}
          aria-label={copy.query.label}
          className="h-9 py-2"
        />
      </div>
    </Command>
  )
}

/**
 * Go to, in the Map's top left: a button that opens the find box in its place, under the System card's row; a pick, a
 * click away or Escape folds it back.
 */
export function GoTo({ v, onPick }: { v: EstateView; onPick(sel: string): void }) {
  const [open, setOpen] = useState(false)
  if (!open)
    return (
      <Button
        variant="ghost"
        className={cn(CARD, 'h-9 gap-2 bg-card px-3 text-xs font-normal hover:bg-card')}
        onClick={() => setOpen(true)}
      >
        <Search aria-hidden className="text-muted-foreground" />
        {copy.query.goTo}
      </Button>
    )
  return (
    <div className="w-[min(360px,calc(100vw-48px))]">
      <CanvasQuery
        v={v}
        below
        scoped={false}
        focusOnOpen
        onDismiss={() => setOpen(false)}
        onPick={(s) => {
          setOpen(false)
          onPick(s)
        }}
      />
    </div>
  )
}
