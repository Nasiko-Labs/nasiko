/**
 * History selection's savings in the window (CX-V3 + CX-V3a, `GET /api/observability/finops/context-savings`, the
 * /optimization lead's read under the same query key): what requests could have carried ("without optimization",
 * the pool) against what they sent, per bucket.
 *
 * Proposed on today's server: a bare 404 (`absent`) or a failed read shows nothing here, because /finops/savings above
 * already covers the totals and this tile only adds the history breakdown. The read has no agent filter, so a filtered
 * page says the figure is for every agent.
 */
import { Link } from '@tanstack/react-router'
import { m } from 'motion/react'
import { Panel, PanelSkeleton } from '@nasiko/ui/composites/panel'
import { Swatch } from '@nasiko/ui/composites/chart-marks'
import { copy as optimizationCopy } from '@nasiko/features/optimization/copy'
import type { ContextSavings, SavingsPoint } from '@nasiko/features/optimization/types'
import { seriesAt } from '@nasiko/ui/lib/chart'
import { fmtInt, fmtMoney, fmtPct, fmtShortDay, fmtTokens, fmtUtcHour } from '@nasiko/ui/lib/format'
import { transitions } from '@nasiko/ui/lib/motion'
import type { TokenopsSearch } from '../search'

const SENT = seriesAt(0)
const SAVED = seriesAt(3)

export function ChatHistory({
  data,
  loading,
  filtered,
  search,
}: {
  /** `undefined` while loading; `null` when the server has no such route or the read failed. */
  data: ContextSavings | null | undefined
  loading: boolean
  /** The page has an agent, provider or model filter this read does not apply. */
  filtered: boolean
  /** The window, passed on to /optimization (the two pages share the window keys). */
  search: Pick<TokenopsSearch, 'preset' | 'from' | 'to'>
}) {
  if (loading && data === undefined) return <PanelSkeleton height={200} />
  if (!data || data.reports === 0) return null
  const saved = Math.max(0, data.pool_tokens - data.sent_tokens)
  const pct = data.pool_tokens > 0 ? (saved / data.pool_tokens) * 100 : null
  const costSaved =
    data.pool_cost_usd !== null && data.sent_cost_usd !== null
      ? Math.max(0, data.pool_cost_usd - data.sent_cost_usd)
      : null
  const eligible = data.eligible_requests
  return (
    <Panel
      title="Chat history"
      subtitle={`History selection: the messages a request could carry against what it sent${filtered ? ' · all agents, filters not applied' : ''}`}
      labelledBy="chat-history-title"
      className="h-full"
      actions={
        <Link
          to="/optimization"
          search={{ preset: search.preset, from: search.from, to: search.to }}
          className="text-xs font-medium text-primary-text underline-offset-4 hover:underline pointer-coarse:py-3"
        >
          Your settings <span aria-hidden>→</span>
        </Link>
      }
    >
      <dl className="grid grid-cols-2 gap-px overflow-hidden rounded-md border bg-border sm:grid-cols-4">
        <Figure
          label={optimizationCopy.preview.without}
          value={fmtTokens(data.pool_tokens)}
          title={optimizationCopy.preview.baselineTip}
        />
        <Figure label={optimizationCopy.lead.chart.sent} value={fmtTokens(data.sent_tokens)} />
        <Figure
          label="Saved"
          value={pct === null ? fmtTokens(saved) : `${fmtTokens(saved)} · ${fmtPct(pct)}`}
        />
        <Figure label="Messages dropped" value={fmtInt(data.messages_dropped)} />
      </dl>
      {data.series && data.series.points.length > 1 ? (
        <HistoryBars points={data.series.points} bucket={data.series.bucket} />
      ) : null}
      <p className="text-xs text-muted-foreground">
        {eligible
          ? optimizationCopy.lead.basedOn(
              data.reports,
              eligible,
              Math.round((data.reports / eligible) * 100),
            )
          : null}{' '}
        {costSaved !== null ? `About ${fmtMoney(costSaved)} of input not billed. ` : null}
        Fewer tokens, not necessarily better answers.
      </p>
    </Panel>
  )
}

const W = 600
const H = 72

/**
 * Each bucket's pool as a full bar, its sent part solid and its saved part in the second colour. Decorative: the
 * figures above carry the totals and /optimization has the per-bucket Table view.
 */
function HistoryBars({ points, bucket }: { points: SavingsPoint[]; bucket: 'hour' | 'day' }) {
  const max = Math.max(...points.map((p) => p.pool_tokens), 0)
  if (max <= 0) return null
  const step = W / points.length
  const bar = Math.max(1, step - 2)
  const first = points[0].bucket_start
  const last = points[points.length - 1].bucket_start
  const label = (iso: string) => (bucket === 'hour' ? fmtUtcHour(new Date(iso)) : fmtShortDay(iso))
  return (
    <figure className="m-0 flex flex-col gap-1.5">
      <svg
        aria-hidden
        viewBox={`0 0 ${W} ${H}`}
        preserveAspectRatio="none"
        className="block h-18 w-full"
      >
        {points.map((p, i) => {
          const pool = (p.pool_tokens / max) * H
          const sent = (Math.min(p.sent_tokens, p.pool_tokens) / max) * H
          return (
            <m.g
              key={p.bucket_start}
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ ...transitions.disclosure, delay: i * 0.01 }}
            >
              <rect x={i * step} y={H - pool} width={bar} height={pool - sent} fill={SAVED.fill} />
              <rect x={i * step} y={H - sent} width={bar} height={sent} fill={SENT.fill} />
            </m.g>
          )
        })}
      </svg>
      <figcaption className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
        <span className="flex gap-4">
          <span className="inline-flex items-center gap-1.5">
            <Swatch series={SENT} /> {optimizationCopy.lead.chart.sent}
          </span>
          <span className="inline-flex items-center gap-1.5">
            <Swatch series={SAVED} /> {optimizationCopy.lead.chart.saved}
          </span>
        </span>
        <span className="tabular-nums">
          {label(first)} – {label(last)} (UTC)
        </span>
      </figcaption>
    </figure>
  )
}

function Figure({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <div className="flex min-w-0 flex-col gap-0.5 bg-card p-3" title={title}>
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd className="font-mono text-base font-semibold tabular-nums">{value}</dd>
    </div>
  )
}
