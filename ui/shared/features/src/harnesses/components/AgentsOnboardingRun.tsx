/**
 * One run on the onboarding card, laid out as the deploy page's run log is (DESIGN.md "Stepper"): a header with the
 * device, the share done and one live line over the overall bar; then, side by side once the card is wide enough, the
 * four phases as a vertical log (find → connect → import → send) beside each coding agent's own progress and the
 * history windows. A sub-line only where there is something to say; the icons are the stepper's (done, current,
 * pending, failed), never colour alone. Refused turns keep their Retry under it all.
 */
import { Laptop } from 'lucide-react'
import type { ReactNode } from 'react'
import { Badge } from '@nasiko/ui/primitives/badge'
import { Button } from '@nasiko/ui/primitives/button'
import { Progress } from '@nasiko/ui/primitives/progress'
import { cn } from '@nasiko/ui/lib/utils'
import { copy as pageCopy } from '../copy'
import type { RetryControl } from '../onboarding'
import { harnessStyle } from '../rollup'
import {
  lastMonthIn,
  pendingTurns,
  rejectedTurns,
  runProgress,
  runTotals,
  sentTurns,
  shownPhase,
  type SyncAgent,
  type SyncPhase,
  type SyncRun,
} from '../runs'
import { WindowTimeline } from './AgentsOnboardingWindows'
import { StateIcon, type State } from './bits'

const copy = pageCopy.agentsOnboarding

/** The known harnesses' own names; anything else keeps the server's. */
const agentName = (a: Pick<SyncAgent, 'id' | 'name'>) => {
  const s = harnessStyle(a.id)
  return s.known ? s.name : a.name
}

/** Which of the four phases the run is in: by its phase, or for a failed run by how far its agents got. */
function reached(run: SyncRun, phase: SyncPhase): number {
  switch (phase) {
    case 'awaiting_consent':
    case 'discovering':
      return 0
    case 'connecting':
      return 1
    case 'importing':
      return 2
    case 'sending':
      return 3
    case 'done':
      return 4
    case 'failed': {
      const a = run.agents
      if (!a.some((x) => x.detected)) return 0
      if (!a.some((x) => x.connected)) return 1
      return a.some((x) => x.import.status === 'running' || x.import.status === 'waiting') ? 2 : 3
    }
  }
}

/** One device's run. `waiting` is the whole queue's count when the desktop app reports it, so the card and the app's
 *  Monitor show the same numbers. */
export function RunBlock({
  run,
  retry,
  waiting = null,
}: {
  run: SyncRun
  retry: RetryControl
  waiting?: number | null
}) {
  const pending = waiting ?? pendingTurns(run)
  const value = runProgress(run, pending)
  const phase = shownPhase(run)
  const failed = phase === 'failed'
  const at = reached(run, phase)
  const state = (i: number): State =>
    i < at ? 'done' : i === at ? (failed ? 'failed' : 'current') : 'pending'
  const detected = run.agents.filter((a) => a.detected)
  const running = run.windows.find((w) => w.status === 'running')
  const sent = sentTurns(run)
  // The header's one live line: what it is doing now, and what has gone to Nasiko so far.
  const live =
    phase === 'done'
      ? null
      : [
          phase === 'discovering'
            ? copy.found([])
            : phase === 'connecting'
              ? copy.connecting
              : phase === 'importing' && running
                ? copy.reading(running.label)
                : null,
          sent > 0 && !failed ? copy.sentSoFar(sent) : null,
        ]
          .filter(Boolean)
          .join(' · ')
  // Agents' rows once they are connected (before that each would read "not connected").
  const connected = phase !== 'discovering' && phase !== 'connecting'
  const showTimeline = run.windows.length > 0 && (phase === 'importing' || phase === 'sending')
  return (
    <div className="@container/run flex flex-col gap-4" data-testid="agents-onboarding-run">
      <div className="flex flex-col gap-2">
        <div className="flex items-end justify-between gap-4">
          <div className="flex min-w-0 flex-col gap-0.5">
            <div className="flex min-w-0 items-center gap-2 text-sm">
              <Laptop aria-hidden className="size-4 shrink-0 text-muted-foreground" />
              <span className="truncate font-medium">{run.device_name}</span>
              <Badge variant="outline">{copy.clientLabel(run.client)}</Badge>
            </div>
            {phase === 'done' ? (
              <p className="text-sm">{summary(run)}</p>
            ) : live ? (
              <p className="text-xs text-muted-foreground tabular-nums">{live}</p>
            ) : null}
          </div>
          {/* Finished, the summary says it: no 100% figure or full bar. */}
          {value !== null && !failed && phase !== 'done' ? (
            <span
              aria-hidden
              className="shrink-0 font-mono text-2xl leading-none font-medium tabular-nums"
            >
              {value}
              <span className="text-sm text-muted-foreground">%</span>
            </span>
          ) : null}
        </div>
        {failed || phase === 'done' ? null : (
          <Progress
            aria-label={copy.progressLabel(run.device_name)}
            value={value}
            // Explicit, so what assistive tech hears doesn't depend on the primitive: the number, then "%".
            aria-valuenow={value ?? undefined}
            aria-valuetext={value === null ? undefined : `${value}%`}
            className={cn(
              'h-1.5 motion-reduce:*:transition-none',
              value === null && 'animate-pulse motion-reduce:animate-none',
            )}
          />
        )}
      </div>

      <div className="grid gap-6 border-t pt-4 @[44rem]/run:grid-cols-[minmax(0,15rem)_minmax(0,1fr)]">
        <ol aria-label={copy.stepsLabel(run.device_name)} className="flex flex-col">
          <Step state={state(0)} label={copy.steps.find} last={false}>
            {phase === 'discovering'
              ? null
              : detected.length
                ? copy.found(detected.map(agentName))
                : copy.noneFound}
          </Step>
          <Step state={state(1)} label={copy.steps.connect} last={false}>
            {detected.length > 0 && at >= 1 ? (at > 1 ? copy.connected : copy.connecting) : null}
          </Step>
          <Step state={state(2)} label={copy.steps.import} last={false}>
            {at === 2 && lastMonthIn(run.windows) ? copy.lastMonthIn : null}
          </Step>
          <Step state={state(3)} label={copy.steps.send} last>
            {pending > 0 ? copy.sending(pending) : null}
          </Step>
        </ol>

        <div className="flex min-w-0 flex-col gap-5">
          {connected && run.agents.length ? (
            <Group title={copy.agentsHeading}>
              <ul className="flex flex-col divide-y">
                {run.agents.flatMap((a) => [
                  <AgentRow key={a.id} agent={a} />,
                  a.import.rejected > 0 ? <RejectedRow key={`${a.id}-rejected`} agent={a} /> : null,
                ])}
              </ul>
            </Group>
          ) : null}
          {showTimeline ? (
            <Group title={copy.historyHeading}>
              <WindowTimeline windows={run.windows} device={run.device_name} />
            </Group>
          ) : null}
        </div>
      </div>

      {rejectedTurns(run) > 0 || retry.requested(run) ? (
        <RetrySending run={run} retry={retry} />
      ) : null}
      {failed ? (
        <p role="alert" className="text-sm text-destructive">
          {copy.runFailed(run.error)}
        </p>
      ) : null}
    </div>
  )
}

function summary(run: SyncRun) {
  const t = runTotals(run)
  return copy.summary(t.sessions, t.turns, t.agents.map(agentName))
}

function Group({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-1.5">
      <h3 className="text-xs font-medium text-muted-foreground">{title}</h3>
      {children}
    </section>
  )
}

/** One phase in the log: its icon on the hairline that joins the phases, its name, and a sub-line when there is one. */
function Step({
  state,
  label,
  last,
  children,
}: {
  state: State
  label: string
  last: boolean
  children: string | null
}) {
  return (
    <li
      aria-current={state === 'current' ? 'step' : undefined}
      data-state={state}
      className="relative grid grid-cols-[1rem_1fr] gap-x-3 pb-4 last:pb-0"
    >
      {last ? null : (
        <span
          aria-hidden
          className={cn(
            'absolute top-6 bottom-1 left-2 w-px -translate-x-1/2',
            state === 'done' ? 'bg-success/50' : 'bg-border',
          )}
        />
      )}
      <span className="flex h-5 items-center">
        <StateIcon state={state} />
      </span>
      <div className="min-w-0">
        <p
          className={cn(
            'text-sm leading-5',
            state === 'pending' ? 'text-muted-foreground' : 'font-medium',
          )}
        >
          {label}
          <span className="sr-only">{`: ${copy.stepState[state]}`}</span>
        </p>
        {children ? (
          <p
            className={cn(
              'text-xs tabular-nums',
              state === 'failed' ? 'text-destructive' : 'text-muted-foreground',
            )}
          >
            {children}
          </p>
        ) : null}
      </div>
    </li>
  )
}

/** An agent's row: its state, its name with its colour chip, and where it is on the right; while it imports, its own
 *  share as a small bar (the numbers are in the words, so the bar is decorative). */
function AgentRow({ agent }: { agent: SyncAgent }) {
  const s = copy.agentStatus
  const imp = agent.import
  const row = (icon: StateOrMark, status: string, share: number | null = null) => (
    <Row icon={icon} agent={agent} share={share}>
      {status}
    </Row>
  )
  if (!agent.detected) return row('none', s.notOnComputer)
  if (!agent.connected) return row('none', s.notConnected)
  switch (imp.status) {
    case 'running':
      return row(
        'active',
        s.importing(imp.done, imp.total),
        imp.total ? Math.min(100, (imp.done / imp.total) * 100) : null,
      )
    case 'done':
      return row(
        'done',
        `${s.imported(imp.sessions, imp.turns)}${imp.failed ? s.unreadable(imp.failed) : ''}`,
      )
    case 'failed':
      return row('failed', s.failed(imp.error))
    case 'skipped':
      return row('none', s.notConnected)
    case 'waiting':
      return row('pending', s.waiting)
  }
}

/** An agent's refused turns, with how many each reason covers: a sentence under its row, not a row of its own. */
function RejectedRow({ agent }: { agent: SyncAgent }) {
  const imp = agent.import
  return (
    <li className="flex items-start gap-2 py-1.5 pl-6 text-sm text-destructive">
      {copy.rejected(agentName(agent), imp.rejected, imp.rejected_reasons)}
    </li>
  )
}

type StateOrMark = State | 'active' | 'none'

function Row({
  icon,
  agent,
  share,
  children,
}: {
  icon: StateOrMark
  agent: SyncAgent
  share: number | null
  children: string
}) {
  const quiet = icon === 'none' || icon === 'pending'
  return (
    <li className="flex min-h-9 items-center gap-2 py-1.5 text-sm">
      <StateIcon state={icon} />
      {/* The name never gives way: a long status wraps on the right instead (phones). */}
      <span className={cn('shrink-0', quiet && 'text-muted-foreground')}>
        <HarnessChip agent={agent} />
        {agentName(agent)}
      </span>
      {share !== null ? (
        <span
          aria-hidden
          className="ml-auto h-1 w-16 shrink-0 overflow-hidden rounded-full bg-muted"
        >
          <span
            className="block h-full rounded-full bg-primary transition-[width] duration-500 ease-out motion-reduce:transition-none"
            style={{ width: `${share}%` }}
          />
        </span>
      ) : null}
      <span
        className={cn(
          'min-w-0 text-right text-xs tabular-nums',
          share === null && 'ml-auto',
          icon === 'failed' ? 'text-destructive' : 'text-muted-foreground',
        )}
      >
        {/* Read as "Claude Code, 40 of 123 sessions", not run together. */}
        <span className="sr-only">, </span>
        {children}
      </span>
    </li>
  )
}

/** The harness's colour, as everywhere else on the page (bits.tsx `HarnessLabel`); nothing for an unknown one. */
function HarnessChip({ agent }: { agent: SyncAgent }) {
  const s = harnessStyle(agent.id)
  if (!s.known) return null
  return (
    <span
      aria-hidden
      className="mr-2 inline-block size-2.5 rounded-xs align-[-1px]"
      style={{ background: s.color, borderTop: `2px solid ${s.edge}` }}
    />
  )
}

/** Retry for a run's refused turns; once asked, a line saying the computer will resend them (until it does). */
function RetrySending({ run, retry }: { run: SyncRun; retry: RetryControl }) {
  if (retry.requested(run)) return <p className="text-sm">{copy.retryRequested}</p>
  return (
    <div className="flex flex-col gap-1">
      <div>
        <Button
          size="sm"
          variant="outline"
          disabled={retry.pending(run)}
          onClick={() => retry.send(run)}
        >
          {copy.retrySend}
        </Button>
      </div>
      {retry.failed(run) ? (
        <p role="alert" className="text-sm text-destructive">
          {copy.retryFailed}
        </p>
      ) : null}
    </div>
  )
}
