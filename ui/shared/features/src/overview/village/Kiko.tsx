/**
 * Kiko, the guide, in the bottom-left corner (plans/feat-village-overview.md §4.9; on the right for a day, back on
 * 2026-10-17). Four moods: calm (nothing needs you), a raised hand (a decision is waiting), a sign (a warning), a flare
 * (something is broken). The badge counts what is broken plus what waits on you. The bubble opens to Kiko's right and
 * walks through everything broken, every request, then the top three warnings, one at a time, each with Take me there
 * and its one action; it ends on the Control center, where the same items stay. It opens by itself once per visit, a
 * moment after the village loads, when anything is broken.
 */
import { Crosshair, X } from 'lucide-react'
import { Button } from '@nasiko/ui/primitives/button'
import { cn } from '@nasiko/ui/lib/utils'
import type { Estate } from '../estate/types'
import { KikoArt } from './art'
import { villageCopy as copy } from './copy'
import { ItemAction } from './Log'
import { guideItems, moodOf, type LogItem, type Village } from './model'
import { CARD } from './styles'

export function Kiko({
  log,
  open,
  index,
  onToggle,
  onNext,
  onClose,
  onTake,
  estate,
}: {
  log: Village['log']
  open: boolean
  index: number
  onToggle(): void
  onNext(): void
  onClose(): void
  onTake(item: LogItem): void
  estate: Estate
}) {
  const items = guideItems(log)
  const count = log.fix.length + log.approve.length
  const mood = moodOf(log)
  const it = items.length ? items[index % items.length] : undefined
  return (
    <div className="flex items-end gap-2.5">
      <Button
        variant="ghost"
        className="relative h-24 w-20 shrink-0 rounded-2xl border border-black/5 bg-white p-1.5 shadow-md hover:bg-white"
        aria-label={copy.kiko.label(count)}
        aria-expanded={open}
        onClick={onToggle}
      >
        <KikoArt mood={mood} />
        {count ? (
          <span className="absolute -top-2 -right-2 grid h-5 min-w-5 place-items-center rounded-full border-2 border-card bg-destructive px-1 text-2xs font-bold text-white tabular-nums">
            {count}
          </span>
        ) : null}
      </Button>
      {open ? (
        <div
          data-vp-ignore=""
          className={cn(
            CARD,
            'relative mb-3 w-[min(340px,calc(100vw-140px))] bg-card p-3.5 pt-3 text-sm',
          )}
        >
          <Button
            variant="ghost"
            size="icon-xs"
            className="absolute top-2 right-2 text-muted-foreground"
            aria-label={copy.kiko.hide}
            onClick={onClose}
          >
            <X aria-hidden />
          </Button>
          {it ? (
            <>
              <p className="text-xs text-muted-foreground">
                {copy.kiko.of((index % items.length) + 1, items.length)}
              </p>
              <p className="mt-1 pr-5 font-semibold">{it.request ? copy.kiko.request : it.title}</p>
              <p className="mt-1 text-xs text-muted-foreground">
                {it.request ? it.title : it.detail}
              </p>
              <div className="mt-3 flex flex-wrap gap-1.5">
                <Button size="sm" onClick={() => onTake(it)}>
                  <Crosshair aria-hidden /> {copy.takeMe}
                </Button>
                <ItemAction item={it} estate={estate} />
                {items.length > 1 ? (
                  <Button size="sm" variant="ghost" onClick={onNext}>
                    {copy.kiko.next}
                  </Button>
                ) : null}
              </div>
              <p className="mt-3 border-t pt-2 text-xs text-muted-foreground">{copy.kiko.foot}</p>
            </>
          ) : (
            <>
              <p className="pr-5 font-semibold">{copy.kiko.quietTitle}</p>
              <p className="mt-1 text-xs text-muted-foreground">{copy.kiko.quiet}</p>
            </>
          )}
        </div>
      ) : null}
    </div>
  )
}
