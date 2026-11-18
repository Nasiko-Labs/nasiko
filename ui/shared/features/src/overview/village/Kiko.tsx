/**
 * Kiko, the guide, in the bottom-left corner (plans/feat-village-overview.md §4.9; on the right for a day, back on
 * 2026-10-17). Four moods: calm (nothing needs you), a raised hand (a decision is waiting), a sign (a warning), a flare
 * (something is broken). The badge counts what is broken plus what waits on you. The bubble opens to Kiko's right and
 * walks through everything broken, every request, then the top three warnings, one at a time, each with Take me there
 * and its one action; it ends on the Control center, where the same items stay. With nothing to walk through it holds
 * the setup checklist (the canvases' setup steps): in full while nothing in the range has run, else the steps left
 * under All quiet. It opens by itself once per visit, a moment after the village loads, when anything is broken or
 * nothing has run yet.
 */
import { Link } from '@tanstack/react-router'
import { Circle, CircleCheck, Crosshair, X } from 'lucide-react'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import { cn } from '@nasiko/ui/lib/utils'
import { copy as overviewCopy } from '../copy'
import type { Step } from '../estate/setup'
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
  steps,
  fresh,
}: {
  log: Village['log']
  open: boolean
  index: number
  onToggle(): void
  onNext(): void
  onClose(): void
  onTake(item: LogItem): void
  estate: Estate
  /** The canvases' setup steps, in order. */
  steps: readonly Step[]
  /** Nothing in the range has run: the checklist is the whole message. */
  fresh: boolean
}) {
  const items = guideItems(log)
  const count = log.fix.length + log.approve.length
  const mood = moodOf(log)
  const it = items.length ? items[index % items.length] : undefined
  return (
    <div className="flex items-end gap-2.5">
      <Button
        variant="ghost"
        className="relative h-24 w-20 shrink-0 rounded-2xl border bg-card p-1.5 shadow-md hover:bg-card"
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
          ) : fresh || steps.some((x) => !x.done) ? (
            <Setup steps={steps} fresh={fresh} />
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

/**
 * The setup checklist in Kiko's bubble: every step while nothing has run (the done ones checked), else All quiet over
 * the steps left. The next step says what it brings to life in the village and is the filled button; done is a check
 * and a word, never colour alone.
 */
function Setup({ steps, fresh }: { steps: readonly Step[]; fresh: boolean }) {
  const c = copy.kiko.setup
  const next = steps.find((x) => !x.done)
  const done = steps.filter((x) => x.done).length
  return (
    <>
      <div className="flex items-baseline gap-3 pr-6">
        <p className="font-semibold">{fresh ? c.title : copy.kiko.quietTitle}</p>
        <p className="ml-auto shrink-0 text-xs text-muted-foreground tabular-nums">
          {c.progress(done, steps.length)}
        </p>
      </div>
      <p className="mt-1 text-xs text-muted-foreground">{fresh ? c.intro : c.left}</p>
      <ol className="mt-2 flex flex-col">
        {(fresh ? steps : steps.filter((x) => !x.done)).map((x) => {
          const s = overviewCopy.needs.setup[x.id]
          const isNext = x === next
          return (
            <li key={x.id} className="flex items-start gap-2.5 border-t py-2 first:border-t-0">
              {x.done ? (
                <CircleCheck aria-hidden className="mt-0.5 size-4 shrink-0 text-success" />
              ) : (
                <Circle aria-hidden className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
              )}
              <div className="min-w-0 flex-1">
                <p className={cn('text-sm', x.done ? 'text-muted-foreground' : 'font-medium')}>
                  {s.title}
                  {x.done ? <span className="sr-only">, {c.done}</span> : null}
                </p>
                {isNext ? (
                  <p className="mt-0.5 text-xs text-muted-foreground">{c.place[x.id]}</p>
                ) : null}
              </div>
              {x.done ? null : (
                <Link
                  to={x.to}
                  search={{} as never}
                  aria-label={s.title}
                  {...asButton({ size: 'xs', variant: isNext ? 'default' : 'outline' })}
                >
                  {s.action}
                </Link>
              )}
            </li>
          )
        })}
      </ol>
    </>
  )
}
