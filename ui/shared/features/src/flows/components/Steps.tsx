/**
 * Steps: every agent call in start order, the swimlane's accessible view (plans/feat-flows.md §2 item 3, F3, F15, F20).
 * Calls that ran at the same time under one parent group as "Parallel ×N". A row expands to its input, output (or the
 * reason it failed, in plain words), timing and links; failed rows open by themselves (F15). Calls known only from the
 * trace carry a "from trace" badge (F14); an ambiguous pair says so (O3).
 */
import { Link } from '@tanstack/react-router'
import { useEffect, useRef, useState } from 'react'
import { Disclosure } from '@nasiko/ui/composites/disclosure'
import { Badge } from '@nasiko/ui/primitives/badge'
import { asButton } from '@nasiko/ui/primitives/button'
import { cn } from '@nasiko/ui/lib/utils'
import type { Call } from '../calls'
import { copy, own, reasonFor, stepStatusLabel } from '../copy'
import { fmtDuration } from '../precision'
import type { FanOut } from '../timeline'

const BADGE: Record<string, 'success' | 'info' | 'warning' | 'destructive' | 'muted'> = {
  completed: 'success',
  running: 'info',
  failed: 'destructive',
  awaiting_human: 'warning',
  pending: 'muted',
  resumed: 'muted',
}

export function Steps({
  calls,
  fanOuts,
  flowStartMs,
  now,
  traceLink,
  selected,
  onSelect,
  answer,
}: {
  calls: readonly Call[]
  fanOuts: readonly FanOut[]
  flowStartMs: number
  now: number
  /** Builds "Open this call in the trace" for a span, when the flow's session is known (O1). */
  traceLink: ((spanId: string) => { sessionId: string; trace: string; span: string }) | null
  /** The selected call (`?step=`): its row opens and scrolls into view, without taking focus (F25). */
  selected: string | null
  onSelect: (key: string) => void
  /** "Answer the request" for a call waiting on a human (F20). */
  answer?: (call: Call) => React.ReactNode
}) {
  const failedKeys = calls.filter((c) => c.status === 'failed').map((c) => c.key)
  const [open, setOpen] = useState<ReadonlySet<string>>(() => new Set(failedKeys))
  // A call that fails after the page loaded (a poll, a trace call landing late) opens once too (F15); a row the
  // user collapsed stays collapsed ("adjust state on prop change": no effect).
  const [autoOpened, setAutoOpened] = useState<ReadonlySet<string>>(() => new Set(failedKeys))
  const newlyFailed = failedKeys.filter((k) => !autoOpened.has(k))
  if (newlyFailed.length) {
    setAutoOpened(new Set([...autoOpened, ...newlyFailed]))
    setOpen(new Set([...open, ...newlyFailed]))
  }
  // A new selection opens its row (React's "adjust state on prop change": no effect, no extra render pass).
  const [seen, setSeen] = useState<string | null>(null)
  if (selected !== seen) {
    setSeen(selected)
    if (selected && !open.has(selected)) setOpen(new Set([...open, selected]))
  }
  const rows = useRef(new Map<string, HTMLDivElement>())
  useEffect(() => {
    if (selected) rows.current.get(selected)?.scrollIntoView?.({ block: 'nearest' })
  }, [selected])
  const toggle = (key: string) => {
    // Opening a row selects its call, so its bar lights up (F25).
    if (!open.has(key)) onSelect(key)
    setOpen((prev) => {
      const next = new Set(prev)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }
  const groupOf = new Map<string, FanOut>()
  for (const f of fanOuts) for (const k of f.keys) groupOf.set(k, f)

  const items: (
    | { kind: 'call'; call: Call; n: number }
    | { kind: 'group'; group: FanOut; calls: { call: Call; n: number }[] }
  )[] = []
  const placed = new Set<FanOut>()
  calls.forEach((call, i) => {
    const g = groupOf.get(call.key)
    if (!g) return void items.push({ kind: 'call', call, n: i + 1 })
    if (placed.has(g)) return
    placed.add(g)
    items.push({
      kind: 'group',
      group: g,
      calls: calls.flatMap((c, j) => (g.keys.includes(c.key) ? [{ call: c, n: j + 1 }] : [])),
    })
  })

  const row = (call: Call, n: number) => (
    <div
      key={call.key}
      ref={(el) => {
        if (el) rows.current.set(call.key, el)
        else rows.current.delete(call.key)
      }}
      className={cn(selected === call.key && 'rounded-md bg-accent/40')}
    >
      <StepRow
        call={call}
        n={n}
        open={open.has(call.key)}
        onToggle={() => toggle(call.key)}
        flowStartMs={flowStartMs}
        now={now}
        traceLink={traceLink}
        answer={answer}
      />
    </div>
  )
  return (
    <div className="flex flex-col">
      {items.map((it) =>
        it.kind === 'call' ? (
          row(it.call, it.n)
        ) : (
          <div key={it.group.keys.join()} className="border-t border-border first:border-t-0">
            <p className="pt-2 text-xs font-medium text-muted-foreground">
              {copy.steps.parallel(it.calls.length)}
            </p>
            <div className="border-l border-border pl-3">
              {it.calls.map((c) => row(c.call, c.n))}
            </div>
          </div>
        ),
      )}
    </div>
  )
}

function StepRow({
  call,
  n,
  open,
  onToggle,
  flowStartMs,
  now,
  traceLink,
  answer,
}: {
  call: Call
  n: number
  open: boolean
  onToggle: () => void
  flowStartMs: number
  now: number
  traceLink: ((spanId: string) => { sessionId: string; trace: string; span: string }) | null
  answer?: (call: Call) => React.ReactNode
}) {
  const failed = call.status === 'failed'
  const status = stepStatusLabel(call.status)
  const took = call.endMs === null ? null : fmtDuration(call.endMs - call.startMs, call.exactEnd)
  const link = call.spanId && traceLink ? traceLink(call.spanId) : null
  return (
    <Disclosure
      id={`step-${call.key}`}
      title={`${n}. ${call.agentName ?? copy.panel.unknownAgent}`}
      hint={
        <>
          {status}
          {took
            ? ` · ${took}`
            : call.status === 'awaiting_human'
              ? ''
              : ` · ${copy.steps.stillRunning}`}
          {call.input ? ` · ${call.input}` : ''}
        </>
      }
      open={open}
      onToggle={onToggle}
    >
      <div className="flex flex-col gap-3 pb-4 text-sm">
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant={own(BADGE, call.status) ?? 'muted'}>{status}</Badge>
          {call.source === 'trace' ? <Badge variant="muted">{copy.steps.fromTrace}</Badge> : null}
        </div>
        {call.maybeSame ? <p className="text-muted-foreground">{copy.steps.maybeSame}</p> : null}
        <dl className="grid gap-x-4 gap-y-2 sm:grid-cols-[8rem_1fr]">
          <dt className="text-muted-foreground">{copy.steps.input}</dt>
          <dd className="whitespace-pre-wrap">{call.input ?? copy.steps.noInput}</dd>
          {failed ? (
            <>
              <dt className="text-muted-foreground">{copy.steps.reason}</dt>
              <dd>{reasonFor(call)}</dd>
            </>
          ) : call.output ? (
            <>
              <dt className="text-muted-foreground">{copy.steps.output}</dt>
              <dd className="whitespace-pre-wrap">{call.output}</dd>
            </>
          ) : null}
          <dt className="text-muted-foreground">{copy.steps.timing}</dt>
          <dd className="tabular-nums">
            {copy.steps.startedAfter(`+${fmtDuration(call.startMs - flowStartMs)}`)},{' '}
            {took
              ? copy.steps.took(took)
              : `${copy.steps.stillRunning} (${fmtDuration(now - call.startMs, false)})`}
          </dd>
        </dl>
        {call.status === 'awaiting_human' && answer ? answer(call) : null}
        {call.agentId || link ? (
          <div className="flex flex-wrap gap-2">
            {call.agentId ? (
              <Link
                to="/agents/$agentId"
                params={{ agentId: call.agentId }}
                {...asButton({
                  variant: 'outline',
                  size: 'sm',
                  className: 'pointer-coarse:min-h-11',
                })}
              >
                {copy.steps.openAgent}
              </Link>
            ) : null}
            {link ? (
              <Link
                to="/sessions/$sessionId"
                params={{ sessionId: link.sessionId }}
                search={{ trace: link.trace, span: link.span }}
                {...asButton({
                  variant: 'ghost',
                  size: 'sm',
                  className: 'pointer-coarse:min-h-11',
                })}
              >
                {copy.steps.openInTrace}
              </Link>
            ) : null}
          </div>
        ) : null}
      </div>
    </Disclosure>
  )
}
