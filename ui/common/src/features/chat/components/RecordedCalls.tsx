/**
 * A recorded harness reply's tool calls (v1c §5.7): chips above the assistant text, in array order. More
 * than 5 calls collapse to one summary chip ("N tool calls · K failed") followed by the failed calls'
 * chips (at most 20, E8); expanding shows the first 50 with "Show all N". A chip opens its detail full
 * column width below the chips, never inside a chip (DS10). Detail is always preformatted text.
 */
import { ChevronDown, ChevronUp, Wrench } from 'lucide-react'
import { useId, useMemo, useState } from 'react'
import { Button } from '@/components/ui/button'
import { relTime } from '@/features/agents/format'
import { fmtLatency } from '@/lib/format'
import { useCopy } from '@/lib/useCopy'
import { cn } from '@/lib/utils'
import { announce } from '../announce'
import { copy } from '../copy'
import {
  isEmptyValue,
  sectionText,
  toolCallsToSteps,
  type RecordedCalls as Calls,
  type RecordedStep,
  type SectionLabel,
} from '../recorded'
import { useNow } from '../hooks'
import { tuning } from '../tuning'
import type { ChatMessage } from '../types'
import { StepIcon } from './StepIcon'
import { STEP_CHIP } from './turnStyles'

/** Calls shown before the summary chip takes over, failed chips kept after it, and calls per page (§5.7, E8). */
const INLINE_MAX = 5
const FAILED_PREVIEW_MAX = 20
const EXPANDED_MAX = 50

const LABEL: Record<SectionLabel, string> = {
  Arguments: copy.sectionArguments,
  Output: copy.sectionOutput,
  Error: copy.sectionError,
}

/** Memoized per message (E8): the raw calls map once; details format when they open. */
export function RecordedCallsFor({ message, now: fixed }: { message: ChatMessage; now?: number }) {
  const calls = useMemo(() => toolCallsToSteps(message.metadata), [message.id, message.metadata]) // eslint-disable-line react-hooks/exhaustive-deps
  // Relative times ("5 min ago") move slowly: this reply's own clock, not the page's.
  const now = useNow(tuning.RECORDED_CLOCK_MS, fixed)
  return calls ? <RecordedCalls calls={calls} now={now} /> : null
}

export function RecordedCalls({ calls, now }: { calls: Calls; now: number }) {
  const base = useId()
  const [openAt, setOpenAt] = useState<number | null>(null)
  const [expanded, setExpanded] = useState(false)
  const [showAll, setShowAll] = useState(false)
  const all = calls.steps.map((step, i) => ({ step, i }))
  if (!all.length) return null
  const failed = all.filter((x) => x.step.status === 'error')
  const collapsed = all.length > INLINE_MAX
  const shown = !collapsed
    ? all
    : !expanded
      ? failed.slice(0, FAILED_PREVIEW_MAX)
      : showAll
        ? all
        : all.slice(0, EXPANDED_MAX)
  const open = openAt === null ? undefined : all[openAt]
  const detailId = `${base}-detail`
  return (
    <div className="space-y-2" data-testid="recorded-calls">
      <ul className="flex flex-wrap items-center gap-1.5" aria-label={copy.toolCalls}>
        {collapsed ? (
          <li>
            <Button
              type="button"
              variant="outline"
              size="xs"
              className={STEP_CHIP}
              aria-expanded={expanded}
              onClick={() => setExpanded((v) => !v)}
              data-testid="calls-summary"
            >
              <Wrench className="size-3.5" aria-hidden />{' '}
              {copy.toolCallsSummary(all.length, failed.length)}
              {expanded ? (
                <ChevronUp className="size-3" aria-hidden />
              ) : (
                <ChevronDown className="size-3" aria-hidden />
              )}
            </Button>
          </li>
        ) : null}
        {shown.map(({ step, i }) => (
          <li key={i} className="max-w-full">
            <RecordedChip
              step={step}
              now={now}
              open={openAt === i}
              controls={detailId}
              onToggle={() => setOpenAt(openAt === i ? null : i)}
            />
          </li>
        ))}
        {collapsed && !expanded && failed.length > FAILED_PREVIEW_MAX ? (
          <li className="text-xs text-muted-foreground">
            {copy.moreFailed(failed.length - FAILED_PREVIEW_MAX)}
          </li>
        ) : null}
        {collapsed && expanded && !showAll && all.length > EXPANDED_MAX ? (
          <li>
            <Button
              type="button"
              size="xs"
              variant="ghost"
              className="pointer-coarse:min-h-11"
              onClick={() => setShowAll(true)}
            >
              {copy.showAllCalls(all.length)}
            </Button>
          </li>
        ) : null}
      </ul>
      {open ? <RecordedDetail id={detailId} step={open.step} captured={calls.captured} /> : null}
    </div>
  )
}

function RecordedChip({
  step,
  now,
  open,
  controls,
  onToggle,
}: {
  step: RecordedStep
  now: number
  open: boolean
  controls: string
  onToggle(): void
}) {
  return (
    <Button
      type="button"
      variant="outline"
      size="xs"
      className={cn(STEP_CHIP, 'max-w-full')}
      aria-expanded={open}
      aria-controls={open ? controls : undefined}
      onClick={onToggle}
      data-testid="recorded-chip"
    >
      <StepIcon status={step.status} />
      {/* Only the name shrinks; the status, time and tags stay on one line. */}
      <span className="min-w-0 truncate">{step.name}</span>
      {step.statusWord ? (
        <span
          className={cn(
            'shrink-0 whitespace-nowrap',
            step.status === 'error' && 'text-destructive',
          )}
        >
          · {step.statusWord}
        </span>
      ) : null}
      {step.durationMs !== undefined ? (
        <span className="shrink-0 whitespace-nowrap tabular-nums">
          {fmtLatency(step.durationMs)}
        </span>
      ) : null}
      {step.startedAt ? (
        <span className="shrink-0 whitespace-nowrap tabular-nums">
          · {relTime(step.startedAt, now)}
        </span>
      ) : null}
      {step.association === 'turn' ? (
        <span
          className="shrink-0 rounded bg-muted px-1 text-xs whitespace-nowrap"
          title={copy.inferredTip}
        >
          {copy.inferredTag}
          <span className="sr-only">: {copy.inferredTip}</span>
        </span>
      ) : null}
      {step.association === 'unknown' ? (
        <span className="shrink-0 rounded bg-muted px-1 text-xs whitespace-nowrap">
          {copy.linkUnknown}
        </span>
      ) : null}
    </Button>
  )
}

function RecordedDetail({
  id,
  step,
  captured,
}: {
  id: string
  step: RecordedStep
  captured: boolean
}) {
  return (
    <div
      id={id}
      role="region"
      aria-label={step.name}
      className="space-y-3 rounded-md border border-border bg-muted/40 p-3 text-xs"
      data-testid="recorded-detail"
    >
      {!captured ? (
        <p className="text-muted-foreground">{copy.contentNotCaptured}</p>
      ) : !step.sections.length ? (
        <p className="text-muted-foreground">{copy.noOutput}</p>
      ) : (
        step.sections.map((s) => (
          <section key={s.label} aria-label={LABEL[s.label]} className="space-y-1">
            <h4 className="font-medium">{LABEL[s.label]}</h4>
            {isEmptyValue(s.raw) ? (
              <p className="text-muted-foreground">{copy.noOutput}</p>
            ) : (
              <SectionBody raw={s.raw} />
            )}
          </section>
        ))
      )}
    </div>
  )
}

/** Stringified and cut only here, when the detail is open (E8). */
function SectionBody({ raw }: { raw: unknown }) {
  const { text, cut, full } = useMemo(() => sectionText(raw), [raw])
  const [, run] = useCopy(full)
  const copyFull = async () => announce((await run()) ? copy.copied : copy.copyFailed)
  return (
    <>
      <pre className="max-h-80 overflow-auto rounded bg-background p-2 break-words whitespace-pre-wrap">
        {text}
      </pre>
      {cut ? (
        <p className="flex flex-wrap items-center gap-2 text-muted-foreground">
          {copy.truncatedAt(tuning.SAVED_DETAIL_MAX)}
          <Button
            type="button"
            size="xs"
            variant="outline"
            className="pointer-coarse:min-h-11"
            onClick={() => void copyFull()}
          >
            {copy.copyFullValue}
          </Button>
        </p>
      ) : null}
    </>
  )
}
