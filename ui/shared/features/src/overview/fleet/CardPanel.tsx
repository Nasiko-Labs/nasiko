/**
 * The canvas drawer for a picked Map card or a day of the bill (Ans, 2026-10-17): the insights first (what the card
 * adds up to, how to read its colours, what in it needs a look), then the buttons that go on to the pages. Built like
 * the insight panel (panel/InsightPanel.tsx) so the drawer reads the same whatever was picked; the colour key draws in
 * the board's tones (`fb-tones`). Its rows pick on the board (a problem, an opportunity, a day), never navigate.
 */
import { Link } from '@tanstack/react-router'
import { X } from 'lucide-react'
import type { ReactNode } from 'react'
import type { OptimisationView } from '@nasiko/features/tokenops/optimisation'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import { ScrollArea } from '@nasiko/ui/primitives/scroll-area'
import { fmtMoney, fmtShortDay } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import type { Spend } from '../api'
import { estateCopy } from '../estate/copy'
import type { EstateView } from '../estate/useEstateView'
import { STRONG_BG } from '../map/tones'
import { TargetLink } from '../panel/targets'
import type { Hud } from '../village/model'
import { cardInsight, visibleOpps, type Action, type BoardSel } from './cards'
import { boardCopy } from './copy'
import type { Board, SpendCard } from './model'
import './board.css'

const copy = boardCopy.panel
/** Rows a list in the drawer shows, at most. */
const ROWS = 6

export function CardPanel({
  s,
  v,
  board,
  card,
  dates,
  spend,
  hud,
  mine,
  optimisation,
  onPick,
  onClose,
}: {
  s: BoardSel
  v: EstateView
  board: Board
  card: SpendCard
  dates: readonly string[]
  spend: Spend
  hud: Hud
  mine: boolean
  optimisation: OptimisationView | undefined
  /** A row's selection, as the page's raw `sel`. */
  onPick(sel: string): void
  onClose(): void
}) {
  return (
    <section
      aria-label={copy.label}
      aria-live="polite"
      className="fb-tones flex h-full min-h-0 flex-col"
      data-testid="insight-panel"
    >
      <ScrollArea className="h-full min-h-0 [&>[data-slot=scroll-area-viewport]>div]:block!">
        <div className="pb-4">
          {s.type === 'day' ? (
            <DayView
              index={s.index}
              v={v}
              card={card}
              dates={dates}
              spend={spend}
              onPick={onPick}
              onClose={onClose}
            />
          ) : (
            <CardView
              s={s}
              v={v}
              board={board}
              card={card}
              dates={dates}
              hud={hud}
              mine={mine}
              optimisation={optimisation}
              onPick={onPick}
              onClose={onClose}
            />
          )}
        </div>
      </ScrollArea>
    </section>
  )
}

function Head({ title, sub, onClose }: { title: string; sub: ReactNode; onClose(): void }) {
  return (
    <div className="sticky top-0 z-10 bg-card/95 px-4 pt-3.5 pb-2 backdrop-blur-sm">
      <div className="flex items-center gap-1.5">
        <h3 className="min-w-0 flex-1 truncate text-sm font-semibold">{title}</h3>
        <Button
          variant="ghost"
          size="icon-sm"
          className="-mr-1.5 text-muted-foreground"
          aria-label={copy.close}
          onClick={onClose}
        >
          <X aria-hidden />
        </Button>
      </div>
      <p className="mt-0.5 text-sm">{sub}</p>
    </div>
  )
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="px-4 pt-3">
      <h4 className="mb-1 text-xs font-semibold text-muted-foreground">{title}</h4>
      {children}
    </div>
  )
}

/** A row that picks on the board: a marker, its name, and its number. */
function Row({
  marker,
  name,
  value,
  onClick,
}: {
  marker?: ReactNode
  name: string
  value?: ReactNode
  onClick(): void
}) {
  return (
    <Button
      variant="ghost"
      size="sm"
      className="-mx-2 h-8 w-[calc(100%+1rem)] justify-start gap-2 px-2 font-normal"
      onClick={onClick}
    >
      {marker}
      <span className="min-w-0 flex-1 truncate text-left">{name}</span>
      {value ? <span className="text-xs text-muted-foreground tabular-nums">{value}</span> : null}
    </Button>
  )
}

function KV({ rows }: { rows: [string, ReactNode][] }) {
  return (
    <dl className="grid grid-cols-[1fr_auto] gap-x-4 gap-y-1 text-sm">
      {rows.map(([k, val]) => (
        <div key={k} className="contents">
          <dt className="text-muted-foreground">{k}</dt>
          <dd className="text-right tabular-nums">{val}</dd>
        </div>
      ))}
    </dl>
  )
}

/** The pages a card goes on to: the first is the primary button. */
function Actions({ items }: { items: Action[] }) {
  return (
    <div className="flex flex-wrap gap-2 px-4 pt-4">
      {items.map((a, i) => (
        <Link
          key={a.label}
          to={a.to}
          hash={a.hash}
          {...asButton({ variant: i ? 'outline' : 'default', size: 'sm' })}
        >
          {a.label}
        </Link>
      ))}
    </div>
  )
}

function CardView({
  s,
  v,
  board,
  card,
  dates,
  hud,
  mine,
  optimisation,
  onPick,
  onClose,
}: {
  s: Extract<BoardSel, { type: 'card' }>
  v: EstateView
  board: Board
  card: SpendCard
  dates: readonly string[]
  hud: Hud
  mine: boolean
  optimisation: OptimisationView | undefined
  onPick(sel: string): void
  onClose(): void
}) {
  const i = cardInsight(s.id, v, board, card, hud, optimisation, mine)
  // What in the card needs a look: the open problems on its parts.
  const ids = i.panel
    ? new Set([...v.nodes.values()].filter((n) => n.panel === i.panel).map((n) => n.id))
    : null
  const problems = ids ? v.problems.filter((p) => p.entities.some((x) => ids.has(x))) : []
  return (
    <>
      <Head title={i.title} sub={i.line} onClose={onClose} />
      <div className="flex flex-col gap-1.5 px-4">
        {i.swatches ? (
          <span className="fb-key">
            {i.swatches.map(([cls, label]) => (
              <span key={cls}>
                <i className={cls} />
                {label}
              </span>
            ))}
          </span>
        ) : null}
        <p className="text-xs text-muted-foreground">{i.key}</p>
      </div>
      {s.id === 'teams' ? (
        <Section title={copy.teams}>
          {board.teams.map((t) => (
            <Row
              key={t.id ?? 'none'}
              name={t.id ? t.name : boardCopy.noTeam}
              value={boardCopy.teamNote(t.projected, t.limit, t.hard)}
              onClick={() => (t.id ? onPick(`node:${t.id}`) : onPick('place:tank'))}
            />
          ))}
        </Section>
      ) : null}
      {s.id === 'opportunities' ? (
        <Section title={copy.ways}>
          {visibleOpps(v).map((o) => (
            <Row
              key={o.id}
              name={o.title}
              value={boardCopy.opps.month(boardCopy.usd0(o.saving_usd))}
              onClick={() => onPick(`opp:${o.id}`)}
            />
          ))}
        </Section>
      ) : null}
      {s.id === 'saved' && optimisation?.categories.length ? (
        <Section title={copy.programs}>
          <KV
            rows={optimisation.categories.map((c) => [
              c.label,
              c.savedCost > 0 ? fmtMoney(c.savedCost) : boardCopy.optimizations.off,
            ])}
          />
        </Section>
      ) : null}
      {s.id === 'bill' && card.spikes.length ? (
        <Section title={copy.peaks}>
          {card.spikes.map((d) => (
            <Row
              key={d}
              marker={<span aria-hidden className="fb-dot t-warn" />}
              name={fmtShortDay(dates[d] ?? '')}
              value={fmtMoney(card.series[d] ?? 0)}
              onClick={() => onPick(`day:${d}`)}
            />
          ))}
        </Section>
      ) : null}
      {i.panel ? (
        <Section title={copy.attention}>
          {problems.length ? (
            problems
              .slice(0, ROWS)
              .map((p) => (
                <Row
                  key={p.id}
                  marker={
                    <span aria-hidden className={cn('size-2 rounded-xs', STRONG_BG[p.kind])} />
                  }
                  name={p.title}
                  value={estateCopy.kind[p.kind]}
                  onClick={() => onPick(`problem:${p.id}`)}
                />
              ))
          ) : (
            <p className="text-sm text-muted-foreground">{copy.healthy}</p>
          )}
        </Section>
      ) : null}
      <Actions items={i.actions} />
    </>
  )
}

/** One day of the bill: what it cost against the daily line and the average, how it was governed, its top agent. */
function DayView({
  index,
  v,
  card,
  dates,
  spend,
  onPick,
  onClose,
}: {
  index: number
  v: EstateView
  card: SpendCard
  dates: readonly string[]
  spend: Spend
  onPick(sel: string): void
  onClose(): void
}) {
  const date = dates[index] ?? ''
  const usd = card.series[index] ?? 0
  const mean = card.series.length ? card.series.reduce((n, x) => n + x, 0) / card.series.length : 0
  const own = v.estate.daily?.length
    ? v.estate.daily[v.estate.daily.length - card.series.length + index]
    : undefined
  const top = spend.days.find((d) => d.iso.slice(0, 10) === date)
  const over = card.threshold !== null && usd > card.threshold
  const rows: [string, ReactNode][] = [
    [copy.spend, fmtMoney(usd)],
    ...(card.threshold !== null
      ? [[copy.line, fmtMoney(card.threshold)] as [string, ReactNode]]
      : []),
    [copy.average, fmtMoney(mean)],
    ...(own
      ? ([
          [copy.budgeted, fmtMoney(own.budgeted_usd)],
          [copy.unbudgeted, fmtMoney(own.unbudgeted_usd)],
          [copy.saved, fmtMoney(own.saved_usd)],
        ] as [string, ReactNode][])
      : []),
    ...(top?.topAgent
      ? [
          [
            copy.top,
            `${top.topAgent}${top.topAgentSpend !== null ? ` · ${fmtMoney(top.topAgentSpend)}` : ''}`,
          ] as [string, ReactNode],
        ]
      : []),
  ]
  return (
    <>
      <Head
        title={copy.day(fmtShortDay(date))}
        sub={copy.daySub(
          fmtMoney(usd),
          over && card.threshold !== null ? fmtMoney(card.threshold) : null,
        )}
        onClose={onClose}
      />
      <div className="px-4 pt-1">
        <KV rows={rows} />
      </div>
      <div className="flex flex-wrap gap-2 px-4 pt-4">
        <TargetLink target={{ page: 'tokenops', query: { day: date } }} estate={v.estate}>
          {copy.dayTokenops}
        </TargetLink>
        <TargetLink
          target={{ page: 'sessions', query: { day: date } }}
          estate={v.estate}
          variant="outline"
        >
          {copy.daySessions}
        </TargetLink>
        <Button variant="ghost" size="sm" onClick={() => onPick('card:bill')}>
          {copy.allBill}
        </Button>
      </div>
    </>
  )
}
