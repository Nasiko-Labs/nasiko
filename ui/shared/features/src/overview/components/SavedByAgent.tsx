/**
 * Saved tokens by agent (Home's PR2d panel, on the Overview since 2026-10-19): the agents token optimisation saved the
 * most for over the range, largest first, each name opening the agent's hub, the rest folded into an Other row. Bars
 * are decorative: each row prints its value. "Other" has no bar, since the rest summed would dwarf the named agents.
 */
import { Link } from '@tanstack/react-router'
import { useRef } from 'react'
import { EmptyState } from '@nasiko/ui/composites/state-card'
import { seriesAt } from '@nasiko/ui/lib/chart'
import { fmtTokens } from '@nasiko/ui/lib/format'
import type { SavedByAgent as SavedByAgentData } from '../api'
import { copy } from '../copy'
import { SAVED_BARS } from '../tuning'
import { Card, CardError, CardSkeleton, TOUCH } from './Card'

const SAVED = seriesAt(1)
const ROW = 'grid grid-cols-[minmax(0,9rem)_minmax(0,1fr)_auto] items-center gap-3 text-sm'

export function SavedByAgent({ data, className }: { data: SavedByAgentData; className?: string }) {
  const titleRef = useRef<HTMLHeadingElement>(null)
  const rows = data.rows
  const bars = rows?.slice(0, SAVED_BARS) ?? []
  const rest = rows?.slice(SAVED_BARS) ?? []
  const max = Math.max(0, ...bars.map((b) => b.tokens))
  const bases = new Set(rows?.map((r) => r.basis))
  const basis = bases.size > 1 ? 'mixed' : ([...bases][0] ?? null)
  return (
    <Card
      id="overview-saved"
      title={copy.saved.title}
      to="/tokenops"
      linkLabel={copy.saved.link}
      linkSearch={{ open: 'optimize' }}
      titleRef={titleRef}
      meta={bars.length ? copy.saved.subtitle(basis) : undefined}
      className={className}
    >
      {data.isPending && !rows ? (
        <CardSkeleton rows={SAVED_BARS} />
      ) : !rows ? (
        <CardError what={copy.saved.what} onRetry={data.retry} titleRef={titleRef} />
      ) : !bars.length ? (
        <EmptyState title={copy.saved.none} className="py-6 md:py-6" />
      ) : (
        <div className="flex flex-col gap-2">
          <ol className="m-0 flex list-none flex-col gap-2.5 p-0" aria-label={copy.saved.title}>
            {bars.map((b) => (
              <li key={b.id} className={ROW}>
                <Link
                  to="/agents/$agentId"
                  params={{ agentId: b.id }}
                  search={{}}
                  title={b.name}
                  className={`truncate underline-offset-4 hover:underline ${TOUCH} pointer-coarse:flex pointer-coarse:items-center`}
                >
                  {b.name}
                </Link>
                <span aria-hidden className="h-2.5 w-full">
                  <span
                    className="block h-full rounded-xs"
                    style={{
                      width: `${max > 0 ? Math.max(1, (b.tokens / max) * 100) : 0}%`,
                      background: SAVED.fill,
                      borderTop: `2px solid ${SAVED.edge}`,
                    }}
                  />
                </span>
                <span className="text-right tabular-nums">{fmtTokens(b.tokens)}</span>
              </li>
            ))}
            {rest.length ? (
              <li className={ROW}>
                <span className="text-muted-foreground">{copy.saved.other(rest.length)}</span>
                <span />
                <span className="text-right tabular-nums">
                  {fmtTokens(rest.reduce((n, r) => n + r.tokens, 0))}
                </span>
              </li>
            ) : null}
          </ol>
          {data.capped ? (
            <p className="m-0 text-xs text-muted-foreground">{copy.saved.capped}</p>
          ) : null}
        </div>
      )}
    </Card>
  )
}
