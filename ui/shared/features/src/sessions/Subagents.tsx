/**
 * Subagents (coding-agent import plan R15): a subagent run is its own session, linked to the session that spawned it.
 * The parts the Sessions list, the session trace and the harness ledger share:
 *
 * - `SubagentChip`: "N subagents" on a parent's row.
 * - `RowSubagents`: the expanded row's children, read lazily (`session/list?parent=`) only once it mounts.
 * - `SessionLineage`: on a session page, "Spawned by <parent>" for a child and the Subagents panel for a parent.
 *
 * Every figure here is the child's own (the server counts it once, on the child); a parent's own cost is never
 * replaced by a sum. Older servers send no subagent fields, so none of this renders there.
 */
import { Link } from '@tanstack/react-router'
import { Bot, CornerLeftUp } from 'lucide-react'
import { Panel } from '@nasiko/ui/composites/panel'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button } from '@nasiko/ui/primitives/button'
import { copy as obsCopy } from '@nasiko/features/observability/copy'
import {
  childRun,
  detailSubagents,
  type SubagentRun,
} from '@nasiko/features/observability/sessions'
import { fmtDuration, fmtMoney, fmtTokens, fmtUtcDayTime } from '@nasiko/ui/lib/format'
import { useSessionDetail, useSubagentSessions } from './api'
import type { SessionsSearch } from './search'

const copy = obsCopy.subagents
const LINK = 'text-primary-text underline-offset-4 hover:underline'

export function SubagentChip({ count }: { count: number }) {
  return (
    <Badge variant="outline" className="rounded px-1 text-2xs">
      <Bot className="size-3" aria-hidden />
      {copy.chip(count)}
    </Badge>
  )
}

/** The runs as a list: each one links to its own session page. */
function RunList({
  runs,
  linkSearch,
}: {
  runs: SubagentRun[]
  linkSearch: Partial<SessionsSearch>
}) {
  return (
    <ul aria-label={copy.listLabel(runs.length)} className="m-0 flex list-none flex-col p-0">
      {runs.map((r) => (
        <li
          key={r.sessionId}
          className="grid grid-cols-2 gap-x-3 gap-y-0.5 border-b border-border py-1.5 text-sm last:border-b-0 @[560px]:grid-cols-[minmax(0,1fr)_4rem_5rem_5rem_8.5rem_5rem] @[560px]:items-baseline"
        >
          <Link
            to="/sessions/$sessionId"
            params={{ sessionId: r.sessionId }}
            search={linkSearch}
            className={`col-span-2 truncate font-medium @[560px]:col-span-1 ${LINK}`}
          >
            {r.type ?? copy.untyped}
          </Link>
          <Figure
            label={copy.turns}
            value={r.turns === null ? '—' : r.turns.toLocaleString('en-US')}
          />
          <Figure label={copy.tokens} value={fmtTokens(r.tokens)} />
          <Figure label={copy.cost} value={fmtMoney(r.cost)} />
          <Figure label={copy.started} value={r.startTime ? fmtUtcDayTime(r.startTime) : '—'} />
          <Figure label={copy.duration} value={fmtDuration(r.durationMs)} />
        </li>
      ))}
    </ul>
  )
}

/** A labelled figure: the label shows only where the list is too narrow for its column headings. */
function Figure({ label, value }: { label: string; value: string }) {
  return (
    <span className="text-muted-foreground tabular-nums @[560px]:text-right @[560px]:text-foreground">
      <span className="@[560px]:sr-only">{label}: </span>
      {value}
    </span>
  )
}

function Headings() {
  return (
    <div
      aria-hidden
      className="hidden grid-cols-[minmax(0,1fr)_4rem_5rem_5rem_8.5rem_5rem] gap-x-3 border-b border-border pb-1 text-xs text-muted-foreground @[560px]:grid"
    >
      <span>{copy.type}</span>
      <span className="text-right">{copy.turns}</span>
      <span className="text-right">{copy.tokens}</span>
      <span className="text-right">{copy.cost}</span>
      <span className="text-right">{copy.started}</span>
      <span className="text-right">{copy.duration}</span>
    </div>
  )
}

function Runs({ runs, linkSearch }: { runs: SubagentRun[]; linkSearch: Partial<SessionsSearch> }) {
  if (!runs.length) return <p className="m-0 text-sm text-muted-foreground">{copy.empty}</p>
  return (
    <div className="@container">
      <Headings />
      <RunList runs={runs} linkSearch={linkSearch} />
    </div>
  )
}

/** Mounted only while the row is expanded: that is what keeps the children's read lazy. */
export function RowSubagents({
  sessionId,
  linkSearch,
}: {
  sessionId: string
  linkSearch: Partial<SessionsSearch>
}) {
  const q = useSubagentSessions(sessionId, true)
  return (
    <section aria-label={copy.title} className="flex flex-col gap-1">
      <div className="text-xs text-muted-foreground">
        {copy.title} · {copy.note}
      </div>
      {q.isPending ? (
        <span className="text-sm text-muted-foreground">{copy.loading}</span>
      ) : q.isError ? (
        <span className="flex items-center gap-2 text-sm text-muted-foreground">
          {copy.error}
          <Button size="sm" variant="outline" onClick={() => void q.refetch()}>
            {copy.retry}
          </Button>
        </span>
      ) : (
        <Runs runs={q.data.map(childRun)} linkSearch={linkSearch} />
      )}
    </section>
  )
}

/** "Spawned by <parent>" for a subagent's session; the parent's title when its detail reads, else a short id. */
function SpawnedBy({
  parentId,
  type,
  linkSearch,
}: {
  parentId: string
  type: string | null
  linkSearch: Partial<SessionsSearch>
}) {
  const parent = useSessionDetail(parentId)
  const name = parent.data?.title || copy.parentFallback(parentId)
  return (
    <p className="m-0 flex flex-wrap items-center gap-1.5 text-sm text-muted-foreground">
      <CornerLeftUp className="size-3.5" aria-hidden />
      {copy.spawnedBy}{' '}
      <Link
        to="/sessions/$sessionId"
        params={{ sessionId: parentId }}
        search={linkSearch}
        className={LINK}
      >
        {name}
      </Link>
      {type ? <span>{copy.spawnedAs(type)}</span> : null}
    </p>
  )
}

/**
 * A session page's place in its subagent tree: where it came from, and the subagents it ran. It reads the session
 * detail the page reads (same query), so the route hands it to both session pages (the trace and the ledger).
 * Nothing renders until the detail is in, nor for a session with neither a parent nor subagents.
 */
export function SessionLineage({
  sessionId,
  linkSearch,
}: {
  sessionId: string
  linkSearch: Partial<SessionsSearch>
}) {
  const detail = useSessionDetail(sessionId)
  if (!detail.data) return null
  const d = detailSubagents(detail.data)
  if (!d.parentSessionId && !d.subagents.length) return null
  return (
    <>
      {d.parentSessionId ? (
        <SpawnedBy parentId={d.parentSessionId} type={d.subagentType} linkSearch={linkSearch} />
      ) : null}
      {d.subagents.length ? (
        <Panel title={`${copy.title} · ${d.subagents.length}`} subtitle={copy.note}>
          <Runs runs={d.subagents} linkSearch={linkSearch} />
        </Panel>
      ) : null}
    </>
  )
}
