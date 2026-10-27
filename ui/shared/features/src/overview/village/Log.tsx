/**
 * The builder's log (plans/feat-village-overview.md §4.9): the persistent home of everything Kiko raises, so hiding
 * Kiko never loses anything. It opens in the canvas's right-hand drawer, where a picked part's panel opens too. Four tabs: Fix (broken problems), Approve (requests waiting on you), Watch (problems
 * getting worse) and Upgrades (the estate's opportunities). Each item has Take me there (select and pan) and its one
 * real action: the problem's or opportunity's own page (`TargetLink`), or the chat where the request waits.
 */
import { Link } from '@tanstack/react-router'
import { ArrowUp, Crosshair, Hand, X } from 'lucide-react'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import { ScrollArea } from '@nasiko/ui/primitives/scroll-area'
import { Tabs, TabsList, TabsTrigger } from '@nasiko/ui/primitives/tabs'
import { cn } from '@nasiko/ui/lib/utils'
import type { Estate } from '../estate/types'
import { TargetLink } from '../panel/targets'
import { villageCopy as copy } from './copy'
import { PoseMark } from './Hud'
import { LOG_KINDS, type LogItem, type LogKind, type Village } from './model'
import { CARD } from './styles'

function KindMark({ kind }: { kind: LogKind }) {
  const base = 'grid size-7 shrink-0 place-items-center rounded-lg'
  if (kind === 'fix')
    return (
      <span className={cn(base, 'bg-destructive/10')}>
        <PoseMark pose="down" />
      </span>
    )
  if (kind === 'watch')
    return (
      <span className={cn(base, 'bg-warning/10')}>
        <PoseMark pose="hurt" />
      </span>
    )
  if (kind === 'approve')
    return (
      <span className={cn(base, 'bg-info/10 text-info')}>
        <Hand aria-hidden className="size-4" />
      </span>
    )
  return (
    <span className={cn(base, 'bg-muted text-muted-foreground')}>
      <ArrowUp aria-hidden className="size-4" />
    </span>
  )
}

/** An item's one real action, wherever it lives. */
export function ItemAction({
  item,
  estate,
  primary,
}: {
  item: LogItem
  estate: Estate
  primary?: boolean
}) {
  const variant = primary ? 'default' : 'outline'
  if (item.problem)
    return (
      <TargetLink target={item.problem.action.target} estate={estate} variant={variant}>
        {item.problem.action.label}
      </TargetLink>
    )
  if (item.opp)
    return (
      <TargetLink target={item.opp.action.target} estate={estate} variant={variant}>
        {item.opp.action.label}
      </TargetLink>
    )
  if (item.request)
    return (
      <Link
        to="/chat/$sessionId"
        params={{ sessionId: item.request.sessionId }}
        state={{ waitingRequest: item.request.id } as never}
        {...asButton({ variant: variant, size: 'sm' })}
      >
        {copy.answer}
      </Link>
    )
  return null
}

export function BuildersLog({
  log,
  tab,
  onTab,
  estate,
  onTake,
  onClose,
  className,
}: {
  log: Village['log']
  tab: LogKind
  onTab(k: LogKind): void
  estate: Estate
  onTake(item: LogItem): void
  onClose(): void
  /** The canvas docks it in its right-hand drawer, which brings its own frame. */
  className?: string
}) {
  const items = log[tab]
  return (
    <section
      aria-label={copy.hud.log}
      data-vp-ignore=""
      className={cn(
        CARD,
        'flex max-h-full w-[min(480px,calc(100vw-48px))] flex-col bg-card',
        className,
      )}
    >
      <div className="flex items-center gap-2 px-2.5 pt-2.5">
        <Tabs value={tab} onValueChange={(k) => onTab(k as LogKind)} className="min-w-0 flex-1">
          <TabsList className="max-w-full justify-start overflow-x-auto">
            {LOG_KINDS.map((k) => (
              <TabsTrigger key={k} value={k} className="gap-1.5 px-2.5 text-xs">
                {copy.logTabs[k]}
                <span className="rounded bg-muted px-1.5 text-2xs text-muted-foreground tabular-nums">
                  {log[k].length}
                </span>
              </TabsTrigger>
            ))}
          </TabsList>
        </Tabs>
        <Button variant="ghost" size="icon-sm" aria-label={copy.close} onClick={onClose}>
          <X aria-hidden />
        </Button>
      </div>
      <ScrollArea className="min-h-0 flex-1">
        {items.length ? (
          <ul className="px-2.5 pb-2.5">
            {items.map((it) => (
              <li key={it.key} className="flex gap-2.5 border-b px-1 py-2.5 last:border-b-0">
                <KindMark kind={it.kind} />
                <div className="min-w-0 flex-1">
                  <p className="text-sm font-medium">{it.title}</p>
                  {it.detail ? (
                    <p className="mt-0.5 text-xs text-muted-foreground">{it.detail}</p>
                  ) : null}
                  <div className="mt-2 flex flex-wrap gap-1.5">
                    {it.anchor || it.sel ? (
                      <Button size="sm" variant="outline" onClick={() => onTake(it)}>
                        <Crosshair aria-hidden /> {copy.takeMe}
                      </Button>
                    ) : null}
                    <ItemAction item={it} estate={estate} primary />
                  </div>
                </div>
              </li>
            ))}
          </ul>
        ) : (
          <p className="px-4 py-8 text-center text-sm text-muted-foreground">
            {copy.logEmpty[tab]}
          </p>
        )}
      </ScrollArea>
    </section>
  )
}
