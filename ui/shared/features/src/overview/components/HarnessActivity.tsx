/**
 * Coding harness activity (2026-11-06): what the developers' coding agents ran over the page's range, in three tiles
 * (separate cards like the summary's, user 2026-11-06), each from the harness usage response (coding_agents.rs `HarnessEntry`): the models by share of turns, the subagent
 * runs their sessions spawned, and the tools by share of calls with the share that failed. The Harnesses tile above
 * keeps the money; this card is the behaviour behind it.
 *
 * A column shows only when the server sent its numbers; one that sends model names alone gets "<harness> mostly runs
 * <model>", no share claimed. With nothing connected, or nothing run in the range, the card is left out: the
 * Harnesses tile above already says so (user 2026-11-06). While Home's coding harness preview plays, `PreviewActivity`
 * shows it on sample numbers above Needs your attention (HarnessPreview.tsx).
 */
import { useRef, type CSSProperties, type ReactNode } from 'react'
import { Card as Tile } from '@nasiko/ui/primitives/card'
import { HARNESSES } from '@nasiko/features/harnesses/constants'
import { seriesAt, type Series } from '@nasiko/ui/lib/chart'
import { fmtInt, fmtPct } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import { harnessDetails, type HarnessSummary } from '../api'
import { copy } from '../copy'
import { usePreviewSample } from '../preview'
import { Card, CardError, CardSkeleton } from './Card'
import { Sampled } from './HarnessPreview'

const known = new Map<string, string>(HARNESSES.map((h) => [h.id, h.name]))
const nameOf = (id: string) => known.get(id) ?? id
const ROW = 'grid grid-cols-[minmax(0,8rem)_minmax(0,1fr)_auto] items-center gap-3 text-sm'

function Column({ title, note, children }: { title: string; note: string; children: ReactNode }) {
  return (
    <Tile className="min-w-0 gap-3 p-4">
      <h3 className="flex items-baseline gap-2 text-sm font-medium">
        {title}
        <span className="text-xs font-normal text-muted-foreground">{note}</span>
      </h3>
      {children}
    </Tile>
  )
}

/** Name, a bar to scale with the column's largest, and the share. */
function Bars({
  rows,
  series,
  label,
}: {
  rows: readonly { name: string; share: number; muted?: boolean }[]
  series: Series
  label: string
}) {
  const max = Math.max(0, ...rows.filter((r) => !r.muted).map((r) => r.share))
  return (
    <ol className="m-0 flex list-none flex-col gap-2.5 p-0" aria-label={label}>
      {rows.map((r) => (
        <li key={r.name} className={ROW}>
          <span className={cn('truncate', r.muted && 'text-muted-foreground')} title={r.name}>
            {r.name}
          </span>
          <span aria-hidden className="h-2.5 w-full">
            {r.muted ? null : (
              <span
                className="block h-full rounded-xs bg-(--bar) [border-top:2px_solid_var(--edge)]"
                style={
                  {
                    width: `${max > 0 ? Math.max(1, (r.share / max) * 100) : 0}%`,
                    '--bar': series.fill,
                    '--edge': series.edge,
                  } as CSSProperties
                }
              />
            )}
          </span>
          <span className="text-right text-muted-foreground tabular-nums">{fmtPct(r.share)}</span>
        </li>
      ))}
    </ol>
  )
}

/** Home's coding harness preview, past Today: this card on sample numbers, sliding in above Needs your attention
 *  (user 2026-11-06: there in the preview only; the real card stays under the inbox and fleet health). */
export function PreviewActivity({ days, className }: { days: number; className?: string }) {
  const sample = usePreviewSample()
  return sample ? (
    <Sampled className={cn('slide-in-from-top-2', className)}>
      <HarnessActivity data={sample} days={days} />
    </Sampled>
  ) : null
}

export function HarnessActivity({
  data,
  days,
  className,
}: {
  data: HarnessSummary
  days: number
  className?: string
}) {
  const titleRef = useRef<HTMLHeadingElement>(null)
  const a = copy.activity
  const d = harnessDetails(data.harnesses)
  // Nothing connected (no lines) or nothing run: the Harnesses tile already says so.
  if (!data.isPending && !data.error && !data.harnesses.some((h) => h.turns > 0)) return null
  return (
    <Card
      id="overview-harness-activity"
      title={a.title}
      meta={a.meta(days)}
      to="/harnesses"
      linkLabel={copy.harnesses.link}
      titleRef={titleRef}
      className={className}
    >
      {data.isPending ? (
        <CardSkeleton rows={4} />
      ) : data.error ? (
        <CardError what={a.what} onRetry={data.retry} titleRef={titleRef} />
      ) : (
        // Three tiles from 760 px of card, stacked below, spaced as the summary's.
        <div className="grid gap-3 @[760px]/card:grid-cols-3">
          {d.models.length || d.namesOnly ? (
            <Column title={a.models} note={d.models.length ? a.modelsNote : ''}>
              {d.models.length ? (
                <Bars
                  label={a.models}
                  series={seriesAt(0)}
                  rows={[
                    ...d.models,
                    ...(d.otherModels
                      ? [
                          {
                            name: a.otherModels(d.otherModels.count),
                            share: d.otherModels.share,
                            muted: true,
                          },
                        ]
                      : []),
                  ]}
                />
              ) : d.namesOnly ? (
                <p className="text-sm">
                  {a.namesOnly(nameOf(d.namesOnly.harness), d.namesOnly.name)}
                </p>
              ) : null}
            </Column>
          ) : null}
          {d.subagentRuns !== null ? (
            <Column title={a.subagents} note={a.subagentsNote}>
              <p className="text-3xl font-semibold tracking-tight tabular-nums">
                {fmtInt(d.subagentRuns)}
              </p>
              {d.subagentsBy.length ? (
                <ul className="flex flex-col gap-1 text-sm">
                  {d.subagentsBy.map((s) => (
                    <li key={s.harness} className="flex justify-between gap-3">
                      <span className="truncate">{nameOf(s.harness)}</span>
                      <span className="text-muted-foreground tabular-nums">{fmtInt(s.runs)}</span>
                    </li>
                  ))}
                </ul>
              ) : null}
            </Column>
          ) : null}
          {d.tools.length ? (
            <Column title={a.tools} note={a.toolsNote}>
              <Bars label={a.tools} series={seriesAt(2)} rows={d.tools} />
              <p className="text-xs text-muted-foreground tabular-nums">
                {a.calls(fmtInt(d.toolCalls))}
                {d.failedPct ? ` · ${a.failed(fmtPct(d.failedPct))}` : ''}
              </p>
            </Column>
          ) : null}
        </div>
      )}
    </Card>
  )
}
