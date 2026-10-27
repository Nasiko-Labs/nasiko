/**
 * The Drivers section's first row: how concentrated spend is, and what the tokens were made of. Two answers a table
 * can't give at a glance.
 *
 * - Spend concentration: one 100% bar, the top five agents (or workflows) plus Other, in the chart series order, so the
 *   colour follows the entity (DESIGN.md "Charts"). The list under it carries every value in words, so no share relies
 *   on colour; hovering either side lights its pair.
 * - Token mix: prompt against completion for the window, then the cache figures (`cacheShare`: providers count cached
 *   input differently, see attribution.ts) and tokens per operation.
 */
import { m } from 'motion/react'
import { useState } from 'react'
import { Panel, PanelEmpty, PanelSkeleton } from '@nasiko/ui/composites/panel'
import { Swatch } from '@nasiko/ui/composites/chart-marks'
import { seriesAt } from '@nasiko/ui/lib/chart'
import { fmtInt, fmtMoney, fmtPct, fmtTokens } from '@nasiko/ui/lib/format'
import { transitions } from '@nasiko/ui/lib/motion'
import { cn } from '@nasiko/ui/lib/utils'
import { cacheShare, concentrationSlices, TOP, type AttributionRow } from '../attribution'
import type { FinopsDashboardData } from '../types'

export function DriversOverview({
  rows,
  tokens,
  view,
  loading,
}: {
  rows: AttributionRow[] | undefined
  tokens: FinopsDashboardData['token_usage'] | undefined
  view: 'agent' | 'workflow'
  loading: boolean
}) {
  return (
    <div className="grid gap-4 xl:grid-cols-12">
      <div className="min-w-0 xl:col-span-7">
        <Concentration rows={rows} view={view} loading={loading} />
      </div>
      <div className="min-w-0 xl:col-span-5">
        <TokenMix tokens={tokens} loading={loading} />
      </div>
    </div>
  )
}

function Concentration({
  rows,
  view,
  loading,
}: {
  rows: AttributionRow[] | undefined
  view: 'agent' | 'workflow'
  loading: boolean
}) {
  const [lit, setLit] = useState<string | null>(null)
  const slices = concentrationSlices(rows ?? [])
  const top = slices[0]
  const noun = view === 'agent' ? 'agent' : 'workflow'
  return (
    <Panel
      title="Spend concentration"
      subtitle={
        top && top.key !== '__other'
          ? `${top.name} alone is ${fmtPct(top.pct)} of spend${slices.length > 1 ? `; the top ${Math.min(TOP, slices.filter((s) => s.key !== '__other').length)} ${noun}s are ${fmtPct(slices.filter((s) => s.key !== '__other').reduce((s, x) => s + x.pct, 0))}` : ''}`
          : `Share of spend by ${noun}`
      }
      labelledBy="concentration-title"
      className="h-full"
    >
      {loading && !rows ? (
        <PanelSkeleton height={96} />
      ) : slices.length === 0 ? (
        <PanelEmpty title={`No ${noun} spend in this window`} />
      ) : (
        <>
          {/* Decorative: the list below carries every share in words. */}
          <div aria-hidden className="flex h-4 w-full gap-0.5 overflow-hidden rounded-md">
            {slices.map((s, i) => (
              <m.div
                key={s.key}
                initial={{ width: 0 }}
                animate={{ width: `${s.pct}%` }}
                transition={{ ...transitions.disclosure, delay: i * 0.04 }}
                onPointerEnter={() => setLit(s.key)}
                onPointerLeave={() => setLit(null)}
                className={cn(
                  'h-full min-w-0.5 transition-opacity first:rounded-l-md last:rounded-r-md',
                  lit && lit !== s.key && 'opacity-35',
                )}
                style={{ backgroundColor: s.series.fill }}
              />
            ))}
          </div>
          <ul className="grid gap-x-6 gap-y-1 text-xs sm:grid-cols-2">
            {slices.map((s) => (
              <li
                key={s.key}
                onPointerEnter={() => setLit(s.key)}
                onPointerLeave={() => setLit(null)}
                className={cn(
                  'flex items-center justify-between gap-3 rounded-md px-1.5 py-1 transition-colors',
                  lit === s.key && 'bg-muted',
                )}
              >
                <span className="inline-flex min-w-0 items-center gap-1.5">
                  <Swatch series={s.series} />
                  <span className="truncate">{s.name}</span>
                </span>
                <span className="flex shrink-0 gap-3 font-mono tabular-nums">
                  <span className="text-muted-foreground">{fmtMoney(s.cost)}</span>
                  <span className="w-12 text-right font-medium">{fmtPct(s.pct)}</span>
                </span>
              </li>
            ))}
          </ul>
        </>
      )}
    </Panel>
  )
}

function TokenMix({
  tokens,
  loading,
}: {
  tokens: FinopsDashboardData['token_usage'] | undefined
  loading: boolean
}) {
  const io = tokens ? tokens.prompt_tokens + tokens.completion_tokens : 0
  const promptPct = tokens && io > 0 ? (tokens.prompt_tokens / io) * 100 : 0
  const cached = tokens ? cacheShare(tokens.prompt_tokens, tokens.cache_read_tokens) : null
  return (
    <Panel
      title="Token mix"
      subtitle="What the window's tokens were made of"
      labelledBy="token-mix-title"
      className="h-full"
    >
      {loading && !tokens ? (
        <PanelSkeleton height={96} />
      ) : !tokens || io === 0 ? (
        <PanelEmpty title="No tokens in this window" />
      ) : (
        <>
          <div className="flex flex-col gap-1.5">
            <div aria-hidden className="flex h-4 w-full gap-0.5 overflow-hidden rounded-md">
              <m.div
                className="h-full rounded-l-md"
                style={{ backgroundColor: seriesAt(0).fill }}
                initial={{ width: 0 }}
                animate={{ width: `${promptPct}%` }}
                transition={transitions.disclosure}
              />
              <div
                className="h-full flex-1 rounded-r-md"
                style={{ backgroundColor: seriesAt(1).fill }}
              />
            </div>
            <div className="flex justify-between text-xs">
              <span className="inline-flex items-center gap-1.5">
                <Swatch series={seriesAt(0)} /> Prompt{' '}
                <span className="font-mono tabular-nums">{fmtTokens(tokens.prompt_tokens)}</span>
                <span className="text-muted-foreground">({fmtPct(promptPct)})</span>
              </span>
              <span className="inline-flex items-center gap-1.5">
                <Swatch series={seriesAt(1)} /> Completion{' '}
                <span className="font-mono tabular-nums">
                  {fmtTokens(tokens.completion_tokens)}
                </span>
              </span>
            </div>
          </div>
          <dl className="grid grid-cols-3 gap-px overflow-hidden rounded-md border bg-border">
            <MixFigure
              label="Cache read"
              value={fmtTokens(tokens.cache_read_tokens)}
              note={cached === null ? '—' : `${fmtPct(cached)} of input`}
            />
            <MixFigure
              label="Cache write"
              value={fmtTokens(tokens.cache_creation_tokens)}
              note="tokens cached"
            />
            <MixFigure
              label="Per operation"
              value={fmtInt(tokens.avg_tokens_per_operation)}
              note="tokens, average"
            />
          </dl>
        </>
      )}
    </Panel>
  )
}

function MixFigure({ label, value, note }: { label: string; value: string; note: string }) {
  return (
    <div className="flex min-w-0 flex-col gap-0.5 bg-card p-3">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="flex flex-col">
        <span className="font-mono text-base font-semibold tabular-nums">{value}</span>
        <span className="truncate text-2xs text-muted-foreground">{note}</span>
      </dd>
    </div>
  )
}
