/**
 * The selected bucket (design "Bucket selection"; `?slice=`, pushed): its totals, the changes recorded in it with
 * their record links (the same items as a flag's panel, so the Table view loses nothing), and where its day leads
 * (Sessions within 7 days, else TokenOps' day panel). Inline under the lanes.
 */
import { X } from 'lucide-react'
import { Button } from '@nasiko/ui/primitives/button'
import { fmtInt, fmtLatency, fmtMoneyFloor } from '@nasiko/ui/lib/format'
import { copy } from '../copy'
import type { Flag, HubRow } from '../timeline'
import { DayLinks } from './DayLinks'
import { MarkerItem } from './MarkerRail'

export function BucketPanel({
  row,
  flag,
  money,
  unpriced,
  agentId,
  agentName,
  now,
  onClose,
}: {
  row: HubRow
  flag: Flag | null
  /** The viewer may see this agent's numbers (the gate passed and the timeseries loaded). */
  money: boolean
  /** Some calls had no price: spend is a floor. */
  unpriced: boolean
  agentId: string
  agentName: string
  now: Date
  onClose: () => void
}) {
  return (
    <section
      aria-label={copy.bucket.title(row.label)}
      className="space-y-2 rounded-lg border border-border bg-card p-3 text-sm"
    >
      <div className="flex items-center justify-between gap-2">
        <h3 className="m-0 text-sm font-semibold">{copy.bucket.title(row.label)}</h3>
        <Button
          variant="ghost"
          size="sm"
          className="size-7 p-0 pointer-coarse:size-11"
          aria-label={copy.bucket.close}
          onClick={onClose}
        >
          <X className="size-4" aria-hidden />
        </Button>
      </div>
      {money && !row.before ? (
        <dl className="m-0 grid grid-cols-3 gap-2 text-xs">
          <div>
            <dt className="text-muted-foreground">{copy.lanes.spend}</dt>
            <dd className="m-0 font-medium tabular-nums">{fmtMoneyFloor(row.spend, unpriced)}</dd>
          </div>
          <div>
            <dt className="text-muted-foreground">{copy.lanes.requests}</dt>
            <dd className="m-0 font-medium tabular-nums">{fmtInt(row.requests)}</dd>
          </div>
          <div>
            <dt className="text-muted-foreground">{copy.lanes.p95}</dt>
            <dd className="m-0 font-medium tabular-nums">{fmtLatency(row.p95)}</dd>
          </div>
        </dl>
      ) : row.before ? (
        <p className="m-0 text-xs text-muted-foreground">{copy.lanes.notDeployedYet}</p>
      ) : null}
      {flag ? (
        <ul className="m-0 list-none space-y-2 p-0">
          {flag.markers.map((m) => (
            <MarkerItem key={m.id} m={m} agentId={agentId} />
          ))}
        </ul>
      ) : (
        <p className="m-0 text-xs text-muted-foreground">{copy.bucket.noMarkers}</p>
      )}
      <DayLinks row={row} agentId={agentId} agentName={agentName} now={now} />
    </section>
  )
}
