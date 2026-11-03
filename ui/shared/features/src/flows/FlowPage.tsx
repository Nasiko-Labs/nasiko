/**
 * One flow (plans/feat-flows.md §2, §2a; T3): header with its links, the narrative (F5), six KPI tiles (F6), the
 * swimlane panel (F2, T2) or the Call card for a lone call (F12), then Steps.
 *
 * Data: `useFlowView` (record polled by the shown status, F13; trace calls merged in, F14). The status shown can
 * differ from the server's (A4, O5); when it does, a line says why. Open trace and Open chat need the flow's chat
 * session (O1). The page's clock ticks only while the flow moves.
 */
import { Link, useNavigate } from '@tanstack/react-router'
import { useEffect, useRef, useState } from 'react'
import { Announcer } from '@nasiko/ui/composites/announcer'
import { useAnnounce } from '@nasiko/ui/composites/announce'
import { CopyButton } from '@nasiko/ui/composites/copy-button'
import { KpiTile } from '@nasiko/ui/composites/kpi-tile'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { PageLoader } from '@nasiko/ui/composites/page-loader'
import { PrevNext } from '@nasiko/ui/composites/prev-next'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { Panel, PanelError } from '@nasiko/ui/composites/panel'
import { StateCard } from '@nasiko/ui/composites/state-card'
import { Alert, AlertDescription } from '@nasiko/ui/primitives/alert'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import { Toggle } from '@nasiko/ui/primitives/toggle'
import { ToggleGroup, ToggleGroupItem } from '@nasiko/ui/primitives/toggle-group'
import { relTime } from '@nasiko/features/agents/format'
import { flowNarrative } from '@nasiko/features/narrative/flows'
import { ApiError } from '@nasiko/data-access/client'
import { useMediaQuery } from '@nasiko/ui/lib/useMediaQuery'
import { useNow } from '@nasiko/app-core/useNow'
import { useSiblings } from '@nasiko/app-core/siblings'
import { isGone, useFlowView } from './api'
import { waitFrom } from './calls'
import { PAUSED_TICK_MS, TICK_MS } from './tuning'
import { useListSearch } from './listSearch'
import { useAnswerTargets } from './answerTargets'
import { AnswerRequest } from './components/AnswerRequest'
import { CallCard } from './components/CallCard'
import { Steps } from './components/Steps'
import { Swimlane } from './components/Swimlane'
import { TimingTable } from './components/TimingTable'
import { copy, reasonFor } from './copy'
import { criticalPath } from './critical'
import { flowKind } from './kind'
import { buildLanes } from './lanes'
import { fmtDuration, fmtWait, hasWholeSeconds } from './precision'
import type { DisplayStatus } from './status'
import { buildAxis, fanOuts } from './timeline'
import { flowSession } from './types'

const STATUS_BADGE: Record<
  DisplayStatus,
  'success' | 'info' | 'warning' | 'destructive' | 'muted'
> = {
  completed: 'success',
  running: 'info',
  finishing: 'info',
  paused: 'warning',
  failed: 'destructive',
  unknown: 'muted',
}
const LIVE = new Set<DisplayStatus>(['running', 'finishing', 'paused'])

export function FlowPage({
  flowId,
  step,
  onStep,
}: {
  flowId: string
  step: string | null
  onStep: (step: string | null) => void
}) {
  return (
    <Announcer>
      <FlowBody flowId={flowId} step={step} onStep={onStep} />
    </Announcer>
  )
}

function FlowBody({
  flowId,
  step,
  onStep,
}: {
  flowId: string
  step: string | null
  onStep: (step: string | null) => void
}) {
  const { query, trace, calls, state } = useFlowView(flowId)
  const session = query.data ? flowSession(query.data.flow) : null
  // One pending-requests read for every "Answer the request" on the page, while a call waits.
  const answerFor = useAnswerTargets(
    session,
    calls.some((c) => c.status === 'awaiting_human'),
  )
  const live = !!state && LIVE.has(state.status)
  // Seconds matter while calls run; a flow waiting on a human counts minutes, so it re-renders rarely.
  const now = useNow(!live ? null : state?.status === 'paused' ? PAUSED_TICK_MS : TICK_MS)
  const announce = useAnnounce()
  const [showCritical, setShowCritical] = useState(true)
  const [view, setView] = useState<'timeline' | 'table' | null>(null)
  // Phones open the panel on its Table (F24): the answer and the calls first, the timeline one tap away.
  const wide = useMediaQuery('(min-width: 640px)')

  // One announcement when a flow this page watched moving comes to rest (F13).
  const prev = useRef<DisplayStatus | null>(null)
  const status = state?.status ?? null
  useEffect(() => {
    if (prev.current && LIVE.has(prev.current) && status && !LIVE.has(status))
      announce(copy.announce(copy.status[status]))
    prev.current = status
  }, [status, announce])

  if (query.isPending) return <PageLoader label={copy.page.loading} />
  // A failed re-read keeps the last good answer (TanStack keeps `data`): only a first read that failed replaces the
  // page, or a flow that's gone since (404) or no longer yours (403), which is no longer re-read.
  if (query.isError && (!query.data || isGone(query.error))) {
    const status = query.error instanceof ApiError ? query.error.status : null
    return (
      <Frame>
        {/* The arrows stay, so a flow that's gone isn't a dead end. */}
        <TopBar root={<FlowsLink />} stepper={<FlowSiblings flowId={flowId} />} />
        {status === 404 ? (
          <StateCard title={copy.page.notFound}>{copy.page.notFoundHint}</StateCard>
        ) : status === 403 && query.data ? (
          <StateCard title={copy.page.noAccess}>{copy.page.noAccessHint}</StateCard>
        ) : (
          <PanelError
            error={query.error}
            onRetry={() => void query.refetch()}
            what={copy.page.what}
          />
        )}
      </Frame>
    )
  }

  const { flow, steps } = query.data
  const shown = state ?? { status: 'completed' as const, markedEarly: false, durationMs: null }
  const startMs = Date.parse(flow.created_at)
  const kind = flowKind(flow)
  const root = flow.root_agent_name ?? copy.panel.unknownAgent
  const title = flow.title || copy.callTo(root)
  const tempo = trace.state !== 'unavailable'
  const exact = !hasWholeSeconds(calls)
  const elapsed = Math.max(0, now - startMs)
  const endMs = live
    ? null
    : shown.durationMs !== null
      ? startMs + shown.durationMs
      : flow.completed_at
        ? Date.parse(flow.completed_at)
        : null

  // A lone call only once the trace has answered: while it loads or after a failed read, its calls are unknown, and
  // the Call card would claim "no other agents" with no Retry in sight.
  const traceSettled =
    trace.state === 'ready' || trace.state === 'unavailable' || trace.state === 'expired'
  const lone = steps.length === 0 && calls.length <= 1 && !live && traceSettled
  // `?step=` that names no call (an old link, a call not drawn yet) selects nothing.
  const selected = step && calls.some((c) => c.key === step) ? step : null
  const lanes = buildLanes(calls)
  // Past 200 calls (F27), or on a phone (F24), the panel opens on its Table; the user's pick wins after that.
  const shownView = view ?? (lanes.tableFirst || !wide ? 'table' : 'timeline')
  const working = shown.status === 'running' || shown.status === 'finishing'
  const axis = buildAxis(calls, startMs, endMs, now, working)
  const groups = fanOuts(calls, now)
  const path = criticalPath(calls)
  const biggest = [...groups].sort((a, b) => b.peak - a.peak)[0]
  const parentName = (key: string) => calls.find((c) => c.key === key)?.agentName ?? null
  const narrative = flowNarrative({
    status: shown.status,
    durationMs: shown.durationMs,
    elapsedMs: elapsed,
    now,
    calls,
    exact,
    fanOut: biggest
      ? { parent: parentName(biggest.parentKey), count: biggest.peak, keys: biggest.keys }
      : null,
    reason: reasonFor,
  })
  const agents = new Set(calls.map((c) => c.agentId ?? c.agentName)).size
  const waiting = calls.find((c) => c.status === 'awaiting_human')
  // Only the statuses that can come early have a note (a failed or unknown flow never does): no line, not an empty one.
  const earlyNote: string | undefined = shown.markedEarly
    ? (copy.markedEarly as Partial<Record<DisplayStatus, string>>)[shown.status]
    : undefined

  const caption = sourceCaption(trace.state, steps.length > 0, calls)
  const traceLink =
    session && tempo ? (span: string) => ({ sessionId: session, trace: flow.flow_id, span }) : null

  return (
    <Frame>
      <TopBar
        root={<FlowsLink />}
        trail={[title]}
        stepper={<FlowSiblings flowId={flow.flow_id} />}
      />
      <PageHeader
        title={title}
        description={
          <span className="inline-flex flex-wrap items-center gap-x-2">
            <span>
              {kind === 'orchestrated' ? copy.page.orchestrator : copy.page.rootAgent(root)}
            </span>
            <span aria-hidden>·</span>
            <span className="font-mono text-code">{flow.flow_id}</span>
            <CopyButton text={flow.flow_id} label={copy.page.copyId} />
          </span>
        }
        actions={
          <>
            {session ? (
              <>
                {tempo ? (
                  <Link
                    to="/sessions/$sessionId"
                    params={{ sessionId: session }}
                    search={{ trace: flow.flow_id }}
                    {...asButton({
                      variant: 'outline',
                      size: 'sm',
                      className: 'pointer-coarse:min-h-11',
                    })}
                  >
                    {copy.actions.openTrace}
                  </Link>
                ) : null}
                <Link
                  to="/chat/$sessionId"
                  params={{ sessionId: session }}
                  {...asButton({
                    variant: 'ghost',
                    size: 'sm',
                    className: 'pointer-coarse:min-h-11',
                  })}
                >
                  {copy.actions.openChat}
                </Link>
              </>
            ) : null}
          </>
        }
      />

      {query.isError ? (
        <Alert>
          <AlertDescription className="flex flex-wrap items-center gap-2">
            {copy.page.stale}
            <Button
              variant="link"
              size="sm"
              className="h-auto p-0"
              onClick={() => void query.refetch()}
            >
              {copy.panel.retry}
            </Button>
          </AlertDescription>
        </Alert>
      ) : null}

      {narrative.length ? (
        <p data-tour="flow-narrative" className="text-base" data-testid="flow-narrative">
          {narrative.map((sentence) => sentence.map((c) => c.text).join('')).join(' ')}
        </p>
      ) : null}

      {waiting ? <AnswerRequest target={answerFor(waiting)} /> : null}

      {earlyNote ? (
        <Alert>
          <AlertDescription>{earlyNote}</AlertDescription>
        </Alert>
      ) : null}

      <div data-tour="flow-kpis" className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-6">
        <KpiTile
          label={copy.kpi.status}
          compact
          value={<Badge variant={STATUS_BADGE[shown.status]}>{copy.status[shown.status]}</Badge>}
        />
        <KpiTile
          label={copy.kpi.duration}
          compact
          value={
            shown.durationMs !== null
              ? fmtDuration(shown.durationMs)
              : live
                ? shown.status === 'paused'
                  ? fmtWait(elapsed)
                  : fmtDuration(elapsed, false)
                : '—'
          }
        />
        <KpiTile label={copy.kpi.started} compact value={relTime(flow.created_at, now)} />
        {steps.length ? <KpiTile label={copy.kpi.steps} compact value={steps.length} /> : null}
        {calls.length ? <KpiTile label={copy.kpi.agents} compact value={agents} /> : null}
        {waiting ? (
          <KpiTile label={copy.kpi.waiting} compact value={fmtWait(now - waitFrom(waiting))} />
        ) : null}
      </div>

      {lone ? (
        <CallCard agent={root} request={flow.title ?? null} />
      ) : (
        <Panel
          title={copy.panel.title}
          labelledBy="flow-timeline"
          subtitle={
            <span className="inline-flex flex-wrap items-center gap-x-2">
              {caption ? <span>{caption}</span> : null}
              {!exact ? <span>{copy.panel.wholeSeconds}</span> : null}
              {trace.overflow ? <span>{copy.panel.more(trace.overflow)}</span> : null}
              {trace.state === 'partial' ? (
                <Button
                  variant="link"
                  size="sm"
                  className="h-auto p-0 text-xs"
                  onClick={trace.retry}
                >
                  {copy.panel.retry}
                </Button>
              ) : null}
            </span>
          }
          actions={
            <>
              <ToggleGroup
                data-tour="flow-view"
                variant="outline"
                size="sm"
                aria-label={copy.view.label}
                value={[shownView]}
                onValueChange={(vs) => {
                  const v = vs[0] as string
                  return v && setView(v as 'timeline' | 'table')
                }}
              >
                <ToggleGroupItem value="timeline" className="pointer-coarse:min-h-11">
                  {copy.view.timeline}
                </ToggleGroupItem>
                <ToggleGroupItem value="table" className="pointer-coarse:min-h-11">
                  {copy.view.table}
                </ToggleGroupItem>
              </ToggleGroup>
              {path.keys ? (
                <Toggle
                  size="sm"
                  variant="outline"
                  pressed={showCritical}
                  onPressedChange={setShowCritical}
                  className="pointer-coarse:min-h-11"
                >
                  {copy.panel.critical}
                </Toggle>
              ) : // Only worth saying when there is a call tree it can't time; a flat sequence has no path to show.
              !live && path.reason === 'timing' && calls.some((c) => c.parentKey) ? (
                <span className="text-xs text-muted-foreground">
                  {copy.panel.criticalUnavailable}: {copy.panel.criticalWhy[path.reason]}
                </span>
              ) : null}
            </>
          }
        >
          {shownView === 'table' ? (
            <>
              {lanes.tableFirst ? (
                <p className="text-sm text-muted-foreground">{copy.panel.tableFirst}</p>
              ) : null}
              <TimingTable
                calls={calls}
                fanOuts={groups}
                critical={path.keys}
                flowStartMs={startMs}
                now={now}
                selected={selected}
                onSelect={onStep}
              />
            </>
          ) : (
            <Swimlane
              model={lanes}
              axis={axis}
              now={now}
              orchestrator={kind === 'orchestrated' ? { startMs, endMs } : null}
              fanOuts={groups}
              critical={showCritical ? path.keys : null}
              label={`${copy.panel.summary(calls.length, agents)} ${narrative
                .flat()
                .map((c) => c.text)
                .join('')}`}
              selected={selected}
              onSelect={onStep}
            />
          )}
        </Panel>
      )}

      {!lone && calls.length ? (
        <Panel title={copy.steps.title} labelledBy="flow-steps" subtitle={copy.steps.subtitle}>
          <Steps
            calls={calls}
            fanOuts={groups}
            flowStartMs={startMs}
            now={now}
            traceLink={traceLink}
            selected={selected}
            onSelect={onStep}
            answer={(c) => <AnswerRequest target={answerFor(c)} size="xs" />}
          />
        </Panel>
      ) : null}
    </Frame>
  )
}

/** "← Flows", back to the list as the user left it (F20), else to its defaults. */
/** Flows, back to the list as it was left (`listSearch.ts`): the breadcrumb's first level. */
function FlowsLink(props: Record<string, unknown>) {
  const search = useListSearch((s) => s.search)
  return (
    // `props`: the breadcrumb's link styling, passed through by its Slot.
    <Link {...props} to="/flows" search={search ?? {}} aria-label={copy.backLabel}>
      {copy.back}
    </Link>
  )
}

/** Previous/Next through the Flows rows this flow was opened from (lib/siblings.ts); nothing on a deep link. */
function FlowSiblings({ flowId, className }: { flowId: string; className?: string }) {
  const navigate = useNavigate()
  const around = useSiblings(flowId)
  if (!around) return null
  // Replace: Back still returns to the list. `?step=` is one flow's call, so it doesn't carry over.
  const go = (id: string | undefined) => {
    if (id)
      void navigate({
        to: '/flows/$flowId',
        params: { flowId: id },
        search: {},
        state: { siblings: around.siblings },
        replace: true,
      })
  }
  return (
    <PrevNext
      noun="flow"
      index={around.index}
      count={around.count}
      onPrev={() => go(around.prev)}
      onNext={() => go(around.next)}
      className={className}
    />
  )
}

function sourceCaption(
  state: 'loading' | 'ready' | 'partial' | 'unavailable' | 'expired',
  hasSteps: boolean,
  calls: readonly { source: string }[],
): string | null {
  if (state === 'unavailable') return null
  if (state === 'loading') return hasSteps ? copy.panel.checking : null
  if (state === 'partial') return copy.panel.partial
  if (state === 'expired')
    return hasSteps ? `${copy.panel.recorded} · ${copy.panel.expired}` : copy.panel.expired
  const fromTrace = calls.some((c) => c.source !== 'recorded')
  if (!fromTrace) return copy.panel.recorded
  return hasSteps ? copy.panel.merged : copy.panel.fromTraces
}

function Frame({ children }: { children: React.ReactNode }) {
  return <div className="mx-auto flex w-full max-w-page flex-col gap-4">{children}</div>
}
