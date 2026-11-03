/**
 * The coding-agent onboarding card (onboarding.ts). In the desktop app it asks first, then shows the desktop's own
 * run; in a browser it shows the command (running it is the consent), then the runs local clients report. Phase
 * changes are spoken through the card's one polite live region.
 */
import { useEffect, useRef } from 'react'
import { AlertTriangle } from 'lucide-react'
import { Announcer } from '@nasiko/ui/composites/announcer'
import { useAnnounce } from '@nasiko/ui/composites/announce'
import { CopyButton } from '@nasiko/ui/composites/copy-button'
import { Button } from '@nasiko/ui/primitives/button'
import { Card } from '@nasiko/ui/primitives/card'
import { copy as pageCopy } from '../copy'
import type { AgentsOnboarding, CardView, DesktopControl } from '../onboarding'
import { rejectedTurns, shownPhase, type SyncRun } from '../runs'
import { RunBlock } from './AgentsOnboardingRun'
import { HarnessOptimization } from './HarnessOptimization'

const copy = pageCopy.agentsOnboarding

export function AgentsOnboardingCard({ onboarding }: { onboarding: AgentsOnboarding }) {
  const { view } = onboarding
  if (!view) return null
  return (
    <Announcer>
      <Card
        className="gap-5 p-5 md:p-6"
        render={
          <section aria-labelledby="agents-onboarding-title" data-testid="agents-onboarding" />
        }
      >
        <CardBody view={view} onboarding={onboarding} />
        <PhaseAnnouncer view={view} />
      </Card>
    </Announcer>
  )
}

function CardBody({ view, onboarding }: { view: CardView; onboarding: AgentsOnboarding }) {
  const { desktop, retry, mode, waiting } = onboarding
  const busy = !!desktop?.busy
  switch (view.kind) {
    case 'command':
      return (
        <>
          <Title>{copy.commandTitle}</Title>
          <Command />
        </>
      )
    case 'ask':
      return (
        <>
          <Title>{copy.title}</Title>
          <p className="max-w-[70ch] text-sm text-muted-foreground">{copy.body}</p>
          <div className="flex flex-wrap gap-2">
            <Button size="sm" disabled={busy} onClick={() => desktop?.act('start')}>
              {copy.yes}
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => desktop?.act('decline')}
            >
              {copy.notNow}
            </Button>
          </div>
          <ActionError desktop={desktop} />
        </>
      )
    case 'declined':
    case 'idle':
      return (
        <>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h2 id="agents-onboarding-title" className="text-sm text-muted-foreground">
              {view.kind === 'declined' ? copy.declined : copy.idle}
            </h2>
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => desktop?.act('start')}
            >
              {copy.findMyAgents}
            </Button>
          </div>
          <ActionError desktop={desktop} />
        </>
      )
    case 'running':
    case 'finished':
      return (
        <>
          <div className="flex flex-col gap-1">
            <Title>{view.kind === 'running' ? copy.runningTitle : finishedTitle(view.runs)}</Title>
            {view.kind === 'running' ? (
              <p className="text-sm text-muted-foreground">{copy.runningBody}</p>
            ) : null}
          </div>
          {/* One block per device, a hairline apart. */}
          <div className="flex flex-col divide-y [&>*]:py-5 [&>*:first-child]:pt-0 [&>*:last-child]:pb-0">
            {view.runs.map((run) => (
              <RunBlock key={run.device_id} run={run} retry={retry} waiting={waiting} />
            ))}
          </div>
          {view.runs.some((r) => r.phase === 'failed') ? (
            desktop ? (
              <TryAgain desktop={desktop} runs={view.runs} />
            ) : mode === 'browser' ? (
              <Command />
            ) : null
          ) : view.kind === 'finished' ? (
            <HarnessOptimization />
          ) : null}
          <ActionError desktop={desktop} />
        </>
      )
  }
}

function finishedTitle(runs: readonly SyncRun[]) {
  if (runs.some((r) => r.phase === 'failed' || r.agents.some((a) => a.import.status === 'failed')))
    return copy.failedTitle
  return runs.some((r) => rejectedTurns(r) > 0) ? copy.rejectedTitle : copy.doneTitle
}

function Title({ children }: { children: string }) {
  return (
    <h2 id="agents-onboarding-title" className="text-base font-semibold">
      {children}
    </h2>
  )
}

/** The CLI command with its Copy button. */
function Command() {
  return (
    <div className="flex flex-col gap-2 text-sm">
      <p className="text-muted-foreground">{copy.commandBody}</p>
      <div className="flex flex-wrap items-center gap-2">
        <pre className="overflow-x-auto rounded-md bg-muted px-3 py-2 text-xs">
          <code>{copy.command}</code>
        </pre>
        <CopyButton text={copy.command} label={copy.copyCommand} />
      </div>
    </div>
  )
}

/** A failed desktop run starts again: Retry when it had turns refused (it re-queues them), else Start. */
function TryAgain({ desktop, runs }: { desktop: DesktopControl; runs: readonly SyncRun[] }) {
  const action = runs.some((r) => rejectedTurns(r) > 0) ? 'retry' : 'start'
  return (
    <div>
      <Button
        size="sm"
        variant="outline"
        disabled={!!desktop.busy}
        onClick={() => desktop.act(action)}
      >
        {copy.tryAgain}
      </Button>
    </div>
  )
}

/** A desktop start or decline that failed: what happened, and Try again repeats it. (A failed Retry says so on its
 *  run.) */
function ActionError({ desktop }: { desktop: DesktopControl | null }) {
  const failed = desktop?.failedAction
  if (!desktop || !failed || failed === 'retry') return null
  return (
    <div role="alert" className="flex flex-wrap items-center gap-2 text-sm text-destructive">
      <AlertTriangle aria-hidden className="size-4" />
      <span>{copy.actionFailed(desktop.error)}</span>
      <Button
        size="sm"
        variant="outline"
        disabled={!!desktop.busy}
        onClick={() => desktop.act(failed)}
      >
        {copy.tryAgain}
      </Button>
    </div>
  )
}

/** Speaks each phase change once: each run's discovering → connecting → importing → sending → done/failed. */
function PhaseAnnouncer({ view }: { view: CardView }) {
  const announce = useAnnounce()
  const message = phaseMessage(view)
  const last = useRef<string | null>(null)
  useEffect(() => {
    if (message === last.current) return
    const first = last.current === null
    last.current = message
    // The card's first state is on screen already: speak changes only.
    if (!first && message) announce(message)
  }, [message, announce])
  return null
}

function phaseMessage(view: CardView): string | null {
  const a = copy.announce
  if (view.kind !== 'running' && view.kind !== 'finished') return null
  return view.runs
    .map((r) => {
      switch (shownPhase(r)) {
        case 'discovering':
          return a.discovering(r.device_name)
        case 'connecting':
          return a.connecting(r.device_name)
        case 'importing':
          return a.importing(r.device_name)
        case 'sending':
          return a.sending(r.device_name)
        case 'done':
          return a.done(r.device_name)
        case 'failed':
          return a.failed(r.device_name)
        case 'awaiting_consent':
          return ''
      }
    })
    .filter(Boolean)
    .join(' ')
}
