/**
 * "What your zip needs" (plans/feat-deploy.md §4.1; design review 4, 6, 15): a muted icon per item until a file is read,
 * then a tick or a cross with a one-line fix. Phones get a compact line above the drop zone that opens itself when a
 * check fails.
 */
import {
  CheckCircle2,
  ChevronRight,
  Container,
  FileCode,
  HardDrive,
  Tag,
  XCircle,
} from 'lucide-react'
import { useState } from 'react'
import { Card } from '@/components/ui/card'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { copy } from '../copy'
import type { ChecklistItem } from '../errors'
import { CHECKLIST_ITEMS as ITEMS, type ItemResult, type ItemResults } from '../zipcheck'

// Before a file is read each row shows what it is about, not an empty circle (which reads as a radio button).
const ITEM_ICON = {
  dockerfile: Container,
  entrypoint: FileCode,
  version: Tag,
  size: HardDrive,
} as const

function ItemRow({
  id,
  result,
  compact = false,
}: {
  id: ChecklistItem
  result: ItemResult
  compact?: boolean
}) {
  const Icon =
    result.state === 'pass' ? CheckCircle2 : result.state === 'fail' ? XCircle : ITEM_ICON[id]
  return (
    // Both lists are focus targets (only one is shown at a time): UploadTab focuses the visible one.
    <li
      id={`zip-item-${compact ? 'compact-' : ''}${id}`}
      tabIndex={-1}
      data-state={result.state}
      className="flex gap-3 rounded-sm py-2 outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 focus:ring-offset-background"
    >
      <Icon
        aria-hidden
        className={cn(
          'mt-0.5 size-4 shrink-0 motion-safe:transition-colors motion-safe:duration-150',
          result.state === 'pass' && 'text-success',
          result.state === 'fail' && 'text-destructive',
          result.state === 'unknown' && 'text-muted-foreground',
        )}
      />
      <div className="min-w-0 text-sm">
        <p className={cn(result.state === 'fail' && 'text-destructive')}>
          {copy.deploy.items[id]}
          <span className="sr-only">: {copy.deploy.itemState[result.state]}</span>
        </p>
        {result.detail && result.state !== 'pass' ? (
          <p className="text-xs text-muted-foreground">{result.detail}</p>
        ) : null}
      </div>
    </li>
  )
}

export function Requirements({
  items,
  advisory,
  title = copy.deploy.needs,
  note,
}: {
  items: ItemResults
  advisory: boolean
  title?: string
  note?: string
}) {
  return (
    <Card className="gap-2 p-4" data-testid="zip-requirements">
      <h2 className="text-sm font-semibold">{title}</h2>
      <ul className="divide-y">
        {ITEMS.map((id) => (
          <ItemRow key={id} id={id} result={items[id]} />
        ))}
      </ul>
      {advisory ? <p className="text-xs text-muted-foreground">{copy.deploy.advisory}</p> : null}
      {note ? <p className="text-xs text-muted-foreground">{note}</p> : null}
    </Card>
  )
}

/** Phones (design review 15): one line that expands, and opens itself when a check fails. */
/** `openSignal` changes on each blocked Deploy: the list opens so the failing row can take focus. */
export function RequirementsCompact({
  items,
  advisory,
  openSignal = 0,
}: {
  items: ItemResults
  advisory: boolean
  openSignal?: number
}) {
  const failing = ITEMS.some((id) => items[id].state === 'fail')
  const [open, setOpen] = useState(failing)
  // Opens itself when a failure first appears; the user can still close it (never a locked-open trigger).
  const [wasFailing, setWasFailing] = useState(failing)
  if (failing !== wasFailing) {
    setWasFailing(failing)
    if (failing) setOpen(true)
  }
  const [seenSignal, setSeenSignal] = useState(openSignal)
  if (openSignal !== seenSignal) {
    setSeenSignal(openSignal)
    setOpen(true)
  }
  return (
    <Collapsible open={open} onOpenChange={setOpen} className="rounded-lg border px-3">
      <CollapsibleTrigger asChild>
        <Button
          variant="ghost"
          className="min-h-11 w-full justify-start gap-2 px-0 text-left text-xs font-normal text-muted-foreground hover:bg-transparent"
        >
          <ChevronRight
            aria-hidden
            className={cn('size-4 shrink-0 transition-transform', open && 'rotate-90')}
          />
          {copy.deploy.needsCompact}
        </Button>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <ul className="divide-y pb-2">
          {ITEMS.map((id) => (
            <ItemRow key={id} id={id} compact result={items[id]} />
          ))}
        </ul>
        {advisory ? (
          <p className="pb-2 text-xs text-muted-foreground">{copy.deploy.advisory}</p>
        ) : null}
      </CollapsibleContent>
    </Collapsible>
  )
}
