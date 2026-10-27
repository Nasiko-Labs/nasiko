/**
 * The marker rail above the first lane (design "Marker visuals", "Lanes"; eng "Markers"). One flag per bucket that has
 * recorded changes, at the oldest change's exact time, placed as a percentage of the width (the lanes' equal bucket
 * slots, `lane-band.ts`), so it lines up with the bars at any width. A click or Enter selects the flag's bucket and opens its panel: each record with its
 * link, and that day's Sessions (or TokenOps beyond Sessions' 7 days), so a marker reaches the day's sessions in two
 * interactions. Escape closes the panel and returns focus to the flag (the Popover). Hover only previews (title).
 */
import { Link } from '@tanstack/react-router'
import {
  Ban,
  CircleDot,
  Gauge,
  Minimize2,
  Package,
  PackageX,
  Play,
  RotateCw,
  Route,
  Square,
  Tag,
  TriangleAlert,
  Undo2,
  Wallet,
  Wrench,
  type LucideIcon,
} from 'lucide-react'
import { createElement } from 'react'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button } from '@nasiko/ui/primitives/button'
import { Popover, PopoverContent, PopoverTrigger } from '@nasiko/ui/primitives/popover'
import { fmtShortDay, fmtUtcDayTime } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import { copy } from '../copy'
import { markerText, type Marker } from '../events'
import type { Flag, HubRow } from '../timeline'
import { RAIL_HEIGHT } from '../tuning'
import { DayLinks } from './DayLinks'

const ICONS: Record<string, LucideIcon> = {
  success: Package,
  failed: PackageX,
  version: Tag,
  crash: TriangleAlert,
  deploy: Package,
  restart: RotateCw,
  start: Play,
  stop: Square,
  rollback: Undo2,
  route_change: Route,
  optimization_change: Gauge,
  compress_change: Minimize2,
  mcp_rule: Wrench,
  budget_warning: Wallet,
  budget_stop: Ban,
}
/** The kind's icon: `createElement`, so no component is picked during render (react-hooks/static-components). */
function MarkerIcon({ sub, className }: { sub: string | null; className: string }) {
  return createElement(sub ? (ICONS[sub] ?? CircleDot) : CircleDot, {
    className,
    'aria-hidden': true,
  })
}

export function MarkerRail({
  flags,
  rows,
  bucket,
  agentId,
  agentName,
  now,
  openIndex,
  focusId,
  onOpen,
}: {
  flags: readonly Flag[]
  rows: readonly HubRow[]
  bucket: 'hour' | 'day'
  agentId: string
  agentName: string
  now: Date
  /** The flag whose panel is open (its bucket index), or null. */
  openIndex: number | null
  /** A `?marker=` id: its flag opens with a focus ring. */
  focusId: string | null
  /** Open (and select the bucket of) a flag, or close with null. */
  onOpen: (index: number | null) => void
}) {
  const n = rows.length
  return (
    <div
      className="relative"
      style={{ height: RAIL_HEIGHT + 4 }}
      aria-label={copy.rail.label}
      role="group"
    >
      {flags.map((f) => {
        const row = rows[f.index]
        if (!row) return null
        const first = f.markers[0]
        if (!first) return null
        const when =
          bucket === 'hour' ? fmtUtcDayTime(new Date(first.at)) : fmtShortDay(new Date(first.at))
        const label = copy.rail.flagLabel(f.markers.length, markerText(first), when)
        const linked = !!focusId && f.markers.some((m) => m.id === focusId)
        return (
          <Popover
            key={f.index}
            open={openIndex === f.index}
            onOpenChange={(open) => onOpen(open ? f.index : null)}
          >
            <PopoverTrigger
              render={
                <Button
                  variant="outline"
                  size="sm"
                  aria-label={label}
                  title={label}
                  className={cn(
                    // A 20 px mark; on coarse pointers an invisible 44 px hit area around it (DESIGN.md targets).
                    "absolute top-0 size-5 -translate-x-1/2 rounded-full p-0 pointer-coarse:after:absolute pointer-coarse:after:-inset-3 pointer-coarse:after:content-['']",
                    f.alert && 'border-destructive text-destructive',
                    linked && 'ring-2 ring-ring ring-offset-1',
                  )}
                  style={{ left: `${((f.index + f.offset) / Math.max(1, n)) * 100}%` }}
                />
              }
            >
              <MarkerIcon sub={f.markers.length > 1 ? null : first.sub} className="size-3" />
              {f.markers.length > 1 ? (
                <Badge
                  variant="secondary"
                  className="absolute -top-1.5 -right-2 h-3.5 min-w-3.5 px-1 text-4xs"
                >
                  {f.markers.length}
                </Badge>
              ) : null}
            </PopoverTrigger>
            <PopoverContent align="center" className="w-80 space-y-3 text-sm">
              <ul className="space-y-2">
                {f.markers.map((m) => (
                  <MarkerItem key={m.id} m={m} agentId={agentId} />
                ))}
              </ul>
              <DayLinks row={row} agentId={agentId} agentName={agentName} now={now} />
            </PopoverContent>
          </Popover>
        )
      })}
    </div>
  )
}

/** One recorded change with its time and record link (the flag's panel and the selected bucket's panel). */
export function MarkerItem({ m, agentId }: { m: Marker; agentId: string }) {
  return (
    <li className="flex gap-2">
      <MarkerIcon
        sub={m.sub}
        className={cn(
          'mt-0.5 size-4 shrink-0',
          m.alert ? 'text-destructive' : 'text-muted-foreground',
        )}
      />
      <div className="min-w-0 space-y-0.5">
        <p className="font-medium">{markerText(m)}</p>
        <p className="text-xs text-muted-foreground">
          <span title={m.approximate ? copy.rail.approximate : undefined}>
            {m.approximate ? '≈ ' : ''}
            {fmtUtcDayTime(new Date(m.at))} UTC
          </span>
          {m.actor ? ` · ${copy.marker.by(m.actor)}` : null}
        </p>
        {m.buildId ? (
          <Link
            to="/builds/$buildId"
            params={{ buildId: m.buildId }}
            className="text-xs text-primary-text underline-offset-4 hover:underline"
          >
            {copy.marker.openBuild}
          </Link>
        ) : m.kind === 'version' ? (
          <Link
            to="/agents/$agentId"
            params={{ agentId }}
            search={{ tab: 'versions' }}
            className="text-xs text-primary-text underline-offset-4 hover:underline"
          >
            {copy.marker.versions}
          </Link>
        ) : null}
      </div>
    </li>
  )
}
