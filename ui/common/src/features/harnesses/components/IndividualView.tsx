/**
 * Individual level (plan §6, G9): full form from the usage endpoint, degraded form from
 * the live fallback. Session links open Sessions only for the viewer themself or a
 * superuser (the session list is own-only for everyone else; Q4); mock mode never links.
 */
import { Link } from '@tanstack/react-router'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { fmtInt, fmtMoney, fmtShortDay, fmtTokens } from '@/lib/format'
import { CopyButton } from '@/components/shared/copy-button'
import { Panel, PanelEmpty } from '@/components/shared/panel'
import { copy } from '../copy'
import { harnessStyle } from '../rollup'
import type { UsageResponse } from '../types'
import { HarnessLabel } from './bits'

export interface SessionItem {
  session_id: string
  harness: string
  started_at: string
  /** Full form: turns + Est. cost. Degraded (chat sessions): messages + tokens. */
  turns?: number
  cost_usd?: number
  messages?: number | null
  tokens?: number | null
}

const when = (iso: string) =>
  new Date(iso).toLocaleString('en-US', {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    timeZone: 'UTC',
  })

export function SessionsList({
  items,
  linkable,
  title,
  note,
  emptyText = 'No harness sessions in this window.',
  initial,
}: {
  items: SessionItem[]
  linkable: boolean
  title: string
  note?: string
  emptyText?: string
  /** Show only the newest `initial` rows until "Show all" (keeps the trend above the fold). */
  initial?: number
}) {
  const [all, setAll] = useState(false)
  const shown = initial && !all ? items.slice(0, initial) : items
  return (
    <Panel title={title} labelledBy="sessions-title" subtitle={note}>
      {!items.length ? (
        <PanelEmpty title={emptyText} />
      ) : (
        <ul className="divide-y divide-border text-sm">
          {shown.map((s) => {
            const label = (
              <>
                <HarnessLabel id={s.harness} />{' '}
                <span className="text-muted-foreground">· {when(s.started_at)} UTC</span>
              </>
            )
            return (
              <li
                key={s.session_id}
                className="flex flex-wrap items-center justify-between gap-2 py-2"
              >
                {linkable ? (
                  <Link
                    to="/sessions/$sessionId"
                    params={{ sessionId: s.session_id }}
                    search={{}}
                    className="inline-flex min-h-8 items-center gap-1 hover:underline"
                  >
                    {label}
                  </Link>
                ) : (
                  <span className="inline-flex min-h-8 items-center gap-1">{label}</span>
                )}
                <span className="text-xs text-muted-foreground tabular-nums">
                  {s.turns !== undefined
                    ? `${fmtInt(s.turns)} turns · ${fmtMoney(s.cost_usd ?? 0)} est.`
                    : `${s.messages ?? '—'} messages · ${s.tokens == null ? '—' : fmtTokens(s.tokens)} tokens`}
                </span>
              </li>
            )
          })}
        </ul>
      )}
      {shown.length < items.length ? (
        <Button size="sm" variant="ghost" className="mt-1 -ml-3" onClick={() => setAll(true)}>
          {copy.showAllSessions(items.length)}
        </Button>
      ) : null}
    </Panel>
  )
}

/** "Not connected: …" with the viewer's connect steps, or a request line about someone else. */
export function ConnectPanel({
  unconnected,
  self,
  name,
  server,
}: {
  unconnected: string[]
  self: boolean
  name: string
  server: string
}) {
  const first = unconnected[0]
  if (!first) return null
  return (
    <Panel
      title={`${copy.notConnected}: ${unconnected.map((h) => harnessStyle(h).name).join(', ')}`}
      labelledBy="connect-title"
    >
      <ConnectHelp self={self} name={name} harness={first} server={server} />
    </Panel>
  )
}

/** Daily activity strip from the series (full form only; no live source, so hidden there). A day is
 *  active when anything ran (active_devs), even if all of it was unpriced; cost only sets the shade. */
export function ActivityStrip({
  series,
  days,
}: {
  series: UsageResponse['series']
  days: string[]
}) {
  const cost = new Map<string, number>()
  const active = new Set<string>()
  const firstDay = days[0]
  const lastDay = days.at(-1)
  for (const p of series) {
    cost.set(p.date, (cost.get(p.date) ?? 0) + p.cost_usd)
    if (p.active_devs > 0) active.add(p.date)
  }
  const max = Math.max(0, ...cost.values())
  const shade = (d: string) => {
    const v = cost.get(d) ?? 0
    if (v > 0)
      return `color-mix(in oklch, var(--chart-1-edge) ${Math.round(25 + 75 * (v / (max || 1)))}%, transparent)`
    return active.has(d)
      ? 'color-mix(in oklch, var(--chart-1-edge) 20%, transparent)'
      : 'var(--muted)'
  }
  return (
    <div className="flex flex-col gap-1">
      <div
        role="img"
        aria-label={`Active on ${days.filter((d) => active.has(d)).length} of ${days.length} days`}
        className="flex flex-wrap gap-0.5"
      >
        {days.map((d) => (
          <span
            key={d}
            title={`${d}: ${!active.has(d) ? 'no activity' : (cost.get(d) ?? 0) > 0 ? `${fmtMoney(cost.get(d) ?? 0)} est.` : 'active, unpriced'}`}
            className="size-3 rounded-sm"
            style={{ background: shade(d) }}
          />
        ))}
      </div>
      {firstDay && lastDay ? (
        <div aria-hidden className="flex justify-between text-xs text-muted-foreground">
          <span>{fmtShortDay(firstDay)}</span>
          <span>{fmtShortDay(lastDay)}</span>
        </div>
      ) : null}
    </div>
  )
}

export function TopModels({ byHarness }: { byHarness: UsageResponse['by_harness'] }) {
  const rows = byHarness.filter((h) => h.top_models.length)
  if (!rows.length) return null
  return (
    <div className="flex flex-col gap-1">
      <h3 className="text-xs font-medium text-muted-foreground">Top models</h3>
      <ul className="flex flex-col gap-1 text-sm">
        {rows.map((h) => (
          <li key={h.harness} className="flex items-center justify-between gap-2">
            <HarnessLabel id={h.harness} />
            <span className="text-muted-foreground">{h.top_models.join(', ')}</span>
          </li>
        ))}
      </ul>
    </div>
  )
}

/** "Not connected" help: the command snippet for the viewer, a request line for everyone else (X6, N14). */
export function ConnectHelp({
  self,
  name,
  harness,
  server,
}: {
  self: boolean
  name: string
  harness: string
  server: string
}) {
  const hname = harnessStyle(harness).name
  if (!self)
    return <p className="text-sm text-muted-foreground">{copy.askToConnect(name, hname)}</p>
  const steps = copy.connectSteps(server, harness)
  return (
    <div className="flex flex-col gap-2 text-sm">
      <pre className="overflow-x-auto rounded-md bg-muted p-3 text-xs">
        <code>{steps.join('\n')}</code>
      </pre>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-xs text-muted-foreground">{copy.connectHint}</p>
        <CopyButton text={steps.join('\n')} label="Copy" />
      </div>
    </div>
  )
}
