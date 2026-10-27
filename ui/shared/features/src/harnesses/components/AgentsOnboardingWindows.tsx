/**
 * The onboarding card's history timeline (contract v2 `windows`, newest first): the finished windows (the first few,
 * then Show all), the window being read, and one line for the rest, so a 40-window run never draws 40 rows. The
 * run log's icons (bits.tsx `StateIcon`).
 */
import { useState } from 'react'
import { Button } from '@nasiko/ui/primitives/button'
import { cn } from '@nasiko/ui/lib/utils'
import { copy as pageCopy } from '../copy'
import { OLDER_WINDOW_LABEL, SYNC_DONE_WINDOWS_SHOWN } from '../constants'
import type { SyncWindow } from '../runs'
import { StateIcon } from './bits'

const copy = pageCopy.agentsOnboarding

const ROW = 'flex min-h-7 items-center gap-2 tabular-nums'

export function WindowTimeline({
  windows,
  device,
}: {
  windows: readonly SyncWindow[]
  device: string
}) {
  const [all, setAll] = useState(false)
  const done = windows.filter((w) => w.status === 'done')
  const running = windows.filter((w) => w.status === 'running')
  const waiting = windows.filter((w) => w.status === 'waiting')
  const hidden = done.length - SYNC_DONE_WINDOWS_SHOWN
  const shownDone = all || hidden <= 0 ? done : done.slice(0, SYNC_DONE_WINDOWS_SHOWN)
  const olderWaiting = waiting.some((w) => w.label === OLDER_WINDOW_LABEL)
  const days = waiting.length - (olderWaiting ? 1 : 0)
  return (
    <div className="flex flex-col gap-1 text-sm">
      <ul aria-label={copy.timelineLabel(device)} className="flex flex-col">
        {shownDone.map((w) => (
          // Labels are unique within a run (contract v2 `windows`).
          <li key={w.label} className={ROW}>
            <StateIcon state="done" />
            <span>{copy.windowDone(w.label, w.sessions)}</span>
          </li>
        ))}
        {running.map((w) => (
          <li key={w.label} className={cn(ROW, 'font-medium')} aria-current="step">
            <StateIcon state="active" />
            <span>{copy.windowRunning(w.label)}</span>
          </li>
        ))}
        {waiting.length ? (
          <li className={cn(ROW, 'text-muted-foreground')}>
            <StateIcon state="pending" />
            <span>{copy.windowsRemaining(days, olderWaiting)}</span>
          </li>
        ) : null}
      </ul>
      {hidden > 0 ? (
        <div>
          <Button
            size="sm"
            variant="link"
            className="h-auto px-0"
            aria-expanded={all}
            onClick={() => setAll((v) => !v)}
          >
            {all ? copy.showFewerWindows : copy.showAllWindows(done.length)}
          </Button>
        </div>
      ) : null}
    </div>
  )
}
