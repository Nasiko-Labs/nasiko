/**
 * Harnesses used (plans/feat-overview.md §8), the KPI row's fourth tile: connected harnesses, active developers, and
 * each harness's share and est. cost over the page's range, from the landing request (the server picks the scope). On OSS
 * (a bare 404) it shows the viewer's own usage and says so, exactly as the Harnesses page does. The share bar uses each
 * harness's chart token, never the accent; the list under it carries the same numbers as text.
 */
import { Link } from '@tanstack/react-router'
import { SquareTerminal } from 'lucide-react'
import type { CSSProperties } from 'react'
import { KpiTile } from '@/components/shared/kpi-tile'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { HARNESSES } from '@/features/harnesses/constants'
import { fmtMoney, fmtPct } from '@/lib/format'
import { cn } from '@/lib/utils'
import type { HarnessSummary } from '../api'
import { copy } from '../copy'
import { SourceFailed, TOUCH } from './Card'
import { TILE, TileLabel } from './Kpis'

const known = new Map<string, { name: string; color: string }>(HARNESSES.map((h) => [h.id, h]))
const colorOf = (id: string) => known.get(id)?.color ?? 'var(--chart-other)'

function pricedOf(data: HarnessSummary) {
  const priced = data.harnesses.filter((h) => !h.unpriced && (h.cost ?? 0) > 0)
  return { priced, total: priced.reduce((n, h) => n + (h.cost ?? 0), 0) }
}

const Dot = () => (
  <span aria-hidden className="text-muted-foreground">
    ·
  </span>
)

/**
 * The first run's Harnesses: one line instead of the KPI tile, so an empty fleet has no loud zero over empty space. The
 * same summary and states as the tile; with harnesses connected (agents aren't needed for them) it keeps the count and
 * the est. cost.
 */
export function HarnessesLine({
  data,
  days,
  className,
}: {
  data: HarnessSummary
  days: number
  className?: string
}) {
  const { total } = pricedOf(data)
  const link = (label: string) => (
    // Underlined: inside a sentence a link can't be told from the text by colour (WCAG 1.4.1).
    <Button
      asChild
      variant="link"
      size="sm"
      className={`h-6 px-0 text-sm underline underline-offset-4 ${TOUCH}`}
    >
      <Link to="/harnesses" search={{} as never}>
        {label}
      </Link>
    </Button>
  )
  return (
    <Card
      asChild
      className={cn(
        'flex-row flex-wrap items-center gap-x-2 gap-y-1 px-4 py-2.5 text-sm',
        className,
      )}
    >
      <section aria-label={copy.harnesses.title} data-testid="overview-harnesses">
        {/* A failed read brings its own warning icon. */}
        {data.error ? null : (
          <SquareTerminal aria-hidden className="size-4 shrink-0 text-muted-foreground" />
        )}
        {data.isPending ? (
          <Skeleton className="h-4 w-56 motion-reduce:animate-none" />
        ) : data.error ? (
          <SourceFailed what={copy.harnesses.what} onRetry={data.retry} />
        ) : data.notVisible ? (
          <span className="text-muted-foreground">{copy.harnesses.notVisible}</span>
        ) : !data.connected ? (
          <>
            <span>{copy.harnesses.noneYet}</span>
            <Dot />
            <span>{link(copy.harnesses.connect)}.</span>
          </>
        ) : (
          <>
            <span className="tabular-nums">{copy.harnesses.connected(data.connected)}</span>
            {data.ownOnly ? (
              <span className="text-muted-foreground">({copy.kpi.yourUsage})</span>
            ) : null}
            {total > 0 ? (
              <>
                <Dot />
                <span className="text-muted-foreground tabular-nums">
                  {copy.harnesses.lineCost(fmtMoney(total), days)}
                </span>
              </>
            ) : null}
            <Dot />
            {link(copy.harnesses.open)}
          </>
        )}
      </section>
    </Card>
  )
}

export function Harnesses({ data, days }: { data: HarnessSummary; days: number }) {
  const { priced, total } = pricedOf(data)
  const share = (h: HarnessSummary['harnesses'][number]) =>
    total > 0 && !h.unpriced ? ((h.cost ?? 0) / total) * 100 : null
  return (
    <KpiTile
      data-testid="overview-harnesses"
      className={TILE}
      label={
        <TileLabel Icon={SquareTerminal} to="/harnesses">
          {copy.kpi.harnesses}
        </TileLabel>
      }
      aside={
        data.ownOnly ? (
          <span className="text-xs text-muted-foreground">{copy.kpi.yourUsage}</span>
        ) : data.activeDevs !== null && data.connected ? (
          <span className="text-xs text-muted-foreground tabular-nums">
            {copy.kpi.devs(data.activeDevs)}
          </span>
        ) : null
      }
      value={
        data.isPending ? (
          <Skeleton className="h-8 w-28 motion-reduce:animate-none" />
        ) : data.error || data.notVisible ? (
          '—'
        ) : (
          <>
            {data.connected}{' '}
            <span className="text-base font-normal text-muted-foreground">
              {copy.kpi.connected}
            </span>
          </>
        )
      }
    >
      {data.isPending ? null : data.error ? (
        <SourceFailed what={copy.harnesses.what} onRetry={data.retry} />
      ) : data.notVisible ? (
        <p className="text-xs text-muted-foreground">{copy.harnesses.notVisible}</p>
      ) : !data.connected ? (
        <p className="text-xs">
          {copy.harnesses.none}{' '}
          <Button asChild variant="link" size="sm" className={`h-auto px-0 text-xs ${TOUCH}`}>
            <Link to="/harnesses" search={{} as never}>
              {copy.harnesses.connect}
            </Link>
          </Button>
        </p>
      ) : (
        <div className="mt-auto flex flex-col gap-2">
          {data.ownOnly ? (
            <p className="text-xs text-muted-foreground">{copy.harnesses.ownOnly}</p>
          ) : null}
          {total > 0 ? (
            <div aria-hidden className="flex h-2 gap-0.5 overflow-hidden rounded-full bg-muted">
              {priced.map((h) => (
                <span
                  key={h.id}
                  className="h-full bg-(--bar)"
                  style={{ width: `${share(h)}%`, '--bar': colorOf(h.id) } as CSSProperties}
                />
              ))}
            </div>
          ) : null}
          <ul className="grid grid-cols-[auto_1fr_auto_auto] items-center gap-x-2 gap-y-0.5 text-xs">
            {data.harnesses.map((h) => {
              const s = share(h)
              return (
                <li key={h.id} className="contents">
                  <span
                    aria-hidden
                    className="size-2.5 rounded-xs bg-(--bar)"
                    style={{ '--bar': colorOf(h.id) } as CSSProperties}
                  />
                  <span className="truncate">{known.get(h.id)?.name ?? h.id}</span>
                  <span className="text-right text-muted-foreground tabular-nums">
                    {s === null ? '' : fmtPct(s)}
                  </span>
                  <span className="text-right text-muted-foreground tabular-nums">
                    {h.unpriced ? copy.harnesses.unpriced : fmtMoney(h.cost ?? 0)}
                  </span>
                </li>
              )
            })}
          </ul>
          <p className="text-[0.6875rem] text-muted-foreground">{copy.harnesses.costNote(days)}</p>
        </div>
      )}
    </KpiTile>
  )
}
