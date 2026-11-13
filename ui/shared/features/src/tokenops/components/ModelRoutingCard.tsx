/**
 * How the LLM router chose the model over the window (NAS-1263,
 * `GET /api/observability/finops/model-routing`).
 *
 * The router picks a model per call through a fixed precedence chain, and until now it only said
 * so in its own logs: nobody could answer "how much of our work did small-talk routing serve"
 * without reading the control plane's log file. This card is that answer.
 *
 * The unit is the **call**, not the trace: one turn classifies once and then reuses that decision,
 * so counting traces would credit a whole conversation to whichever level served its last call.
 * Cost is the other way round — it is only known per trace — so a level's cost covers only the
 * traces it served alone, and the row says so when that is less than all of its work.
 *
 * Absent on a server older than the endpoint (`null`): the card renders nothing rather than
 * claiming that nothing was routed.
 */
import { Panel, PanelSkeleton } from '@nasiko/ui/composites/panel'
import { TableBody, TableCell, TableHead, TableHeader, TableRow } from '@nasiko/ui/primitives/table'
import { fmtInt, fmtMoney, fmtPct } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import type { ModelRoutingData, RoutingBucket } from '../types'
import { CELL, ChartTable, HEAD, NUM, ROW_HEAD, STICKY_HEAD } from './ChartTable'

/** What each precedence level means, in the router's own order (`routing/mod.rs` `RouteSource`). */
const LEVELS: Record<string, string> = {
  pinned: 'Pinned to one model',
  cache_hit: "Reused the conversation's choice",
  small_talk: 'Small talk, served cheaply',
  classified: 'Classifier picked a tier',
  config: "The agent's configured model",
  default: 'Default model',
}

const TIERS: Record<string, string> = {
  tier1: 'Tier 1 (strongest)',
  tier2: 'Tier 2 (middle)',
  tier3: 'Tier 3 (smallest)',
}

export function ModelRoutingCard({
  data,
  loading,
  filtered,
}: {
  /** `undefined` while loading; `null` when the server has no such route. */
  data: ModelRoutingData | null | undefined
  loading: boolean
  /** The page has an agent, provider or model filter; the figures are narrowed to it. */
  filtered: boolean
}) {
  if (loading && data === undefined) return <PanelSkeleton height={200} />
  // No route, or a window whose calls all predate the record: the panels around it already cover
  // the window, and a table of zeros here would read as "the router is off".
  if (!data || data.calls === 0) return null

  const served = data.by_level.filter((b) => b.calls > 0)
  const tiers = data.by_tier.filter((b) => b.calls > 0)
  return (
    <Panel
      title="How models were chosen"
      subtitle={`${fmtInt(data.calls)} routed calls${filtered ? ' · narrowed by the page filters' : ''}`}
      labelledBy="model-routing-title"
      className="h-full"
    >
      <ChartTable className="max-h-none">
        <caption className="sr-only">Routed calls by the level that chose the model</caption>
        <TableHeader className={STICKY_HEAD}>
          <TableRow className="hover:bg-transparent">
            <TableHead scope="col" className={HEAD}>
              How the model was chosen
            </TableHead>
            <TableHead scope="col" className={cn(HEAD, 'text-right')}>
              Calls
            </TableHead>
            <TableHead scope="col" className={cn(HEAD, 'text-right')}>
              Share
            </TableHead>
            <TableHead scope="col" className={cn(HEAD, 'text-right')}>
              Cost
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {served.map((bucket) => (
            <Row key={bucket.key} bucket={bucket} label={LEVELS[bucket.key] ?? bucket.key} />
          ))}
        </TableBody>
      </ChartTable>
      {tiers.length > 0 ? (
        <p className="text-xs text-muted-foreground">
          Tiers drawn:{' '}
          {tiers.map((t) => `${TIERS[t.key] ?? t.key} — ${fmtInt(t.calls)}`).join(' · ')}
        </p>
      ) : null}
      <p className="text-xs text-muted-foreground">
        {fmtInt(data.model_overridden)} of {fmtInt(data.calls)} calls ran a model other than the one
        the agent's own configuration named.
        {data.unrecorded_traces > 0 ? (
          <>
            {' '}
            {fmtInt(data.unrecorded_traces)} traces in this window carry no routing record: calls
            made before it was recorded, and calls that choose no model.
          </>
        ) : null}
      </p>
    </Panel>
  )
}

function Row({ bucket, label }: { bucket: RoutingBucket; label: string }) {
  // A level that only ever shared a trace with another has no cost of its own to report: the trace
  // carries one figure for every call on it, and splitting it between levels would be invention.
  const whole = bucket.attributed_calls >= bucket.calls
  const partial = bucket.attributed_calls > 0 && !whole
  return (
    <TableRow>
      <TableHead scope="row" className={ROW_HEAD}>
        {label}
      </TableHead>
      <TableCell className={cn(CELL, NUM)}>{fmtInt(bucket.calls)}</TableCell>
      <TableCell className={cn(CELL, NUM, 'font-semibold')}>
        {bucket.call_share_pct == null ? '—' : fmtPct(bucket.call_share_pct)}
      </TableCell>
      <TableCell className={cn(CELL, NUM)}>
        {bucket.attributed_calls === 0 ? (
          <span
            className="text-muted-foreground"
            title="Every one of these calls shared a trace with another level, so the trace's cost cannot be split between them"
          >
            —
          </span>
        ) : (
          <span
            title={
              partial
                ? `Covers ${fmtInt(bucket.attributed_calls)} of ${fmtInt(bucket.calls)} calls; the rest shared a trace with another level`
                : undefined
            }
          >
            {fmtMoney(bucket.cost_usd)}
            {partial ? <span className="text-muted-foreground"> (part)</span> : null}
          </span>
        )}
      </TableCell>
    </TableRow>
  )
}
