/** The desktop sync rules (syncStatus.ts, desktop.ts): when the app syncs, the bar's numbers and line, and polling. */
import { describe, expect, it } from 'vitest'
import { doneAgents, importingAgents, mockRun, mockWindows } from '@nasiko/mocks/codingAgents'
import { DESKTOP_IDLE_POLL_MS, DESKTOP_STATE_POLL_MS } from './constants'
import { desktopPollInterval, type DesktopState } from './desktop'
import { runProgress, type LocalRun } from './runs'
import { desktopSync, syncLine } from './syncStatus'

const NOW = Date.parse('2026-03-20T15:00:00Z')
const local = (...args: Parameters<typeof mockRun>): LocalRun => mockRun(...args)
const queue = (pending: number, sending = false) => ({
  pending,
  rejected: 0,
  sending,
  next_attempt_at: null,
})
const desktop = (over: Partial<DesktopState> = {}): DesktopState => ({
  desktop: true,
  consent: 'granted',
  signed_in: true,
  run: null,
  backlog: queue(0),
  ...over,
})

describe('desktopSync', () => {
  it('is idle with no run and an empty queue, after a finished run, and after a failed one', () => {
    expect(desktopSync(desktop()).syncing).toBe(false)
    const done = local('done', doneAgents(), NOW, { windows: mockWindows(NOW, 31) })
    expect(desktopSync(desktop({ run: done })).syncing).toBe(false)
    expect(desktopSync(desktop({ run: local('failed', [], NOW) })).syncing).toBe(false)
  })

  it('importing: windows read over windows, the window being read, turns sent and the whole queue waiting', () => {
    // 10 of the 31 windows done, the 11th being read; Codex sent its 58 turns.
    const run = local('importing', importingAgents(), NOW, { windows: mockWindows(NOW, 10) })
    const sync = desktopSync(desktop({ run, backlog: queue(499) }))
    expect(sync).toMatchObject({ syncing: true, phase: 'importing', value: 32, sent: 58 })
    expect(sync.window).toBe(run.windows[10].label)
    expect(syncLine(sync)).toBe(
      `Syncing coding agents · ${run.windows[10].label} · 58 turns sent · 499 waiting`,
    )
  })

  it('sending: delivered over delivered and waiting, never full', () => {
    // 842 turns imported, none of the run's own left; 158 wait in the whole queue.
    const run = local('sending', doneAgents(), NOW, { windows: mockWindows(NOW, 30) })
    const sync = desktopSync(desktop({ run, backlog: queue(158, true) }))
    expect(sync).toMatchObject({ phase: 'sending', value: 84, sent: 842, waiting: 158 })
    expect(syncLine(sync)).toBe('Syncing coding agents · 842 turns sent · 158 waiting')
    expect(desktopSync(desktop({ run, backlog: queue(0, true) })).value).toBe(99)
  })

  it('the desktop’s own counts win: windows read (not just done) and turns delivered', () => {
    // 10 windows done, 15 read (their turns still on their way); 1,730 turns delivered.
    const run = local('importing', importingAgents(), NOW, {
      windows: mockWindows(NOW, 10),
    })
    const sync = desktopSync(
      desktop({ run: { ...run, windows_read: 15, delivered: 1730 }, backlog: queue(499) }),
    )
    expect(sync).toMatchObject({ value: 48, sent: 1730 })
    expect(syncLine(sync)).toBe(
      `Syncing coding agents · ${run.windows[10].label} · 1,730 turns sent · 499 waiting`,
    )
  })

  it('the card shows the same number for the same run and queue', () => {
    const run = local('sending', doneAgents(), NOW)
    expect(runProgress(run, 158)).toBe(desktopSync(desktop({ run, backlog: queue(158) })).value)
  })

  it('background delivery (no active run): indeterminate, only what waits', () => {
    const done = local('done', doneAgents(), NOW, { windows: mockWindows(NOW, 31) })
    const queued = desktopSync(desktop({ run: done, backlog: queue(499) }))
    expect(queued).toMatchObject({ syncing: true, phase: 'sending', value: null, sent: null })
    expect(syncLine(queued)).toBe('Syncing coding agents · 499 waiting')
    // A delivery pass with nothing counted yet still syncs.
    const passing = desktopSync(desktop({ backlog: queue(0, true) }))
    expect(passing.syncing).toBe(true)
    expect(syncLine(passing)).toBe('Syncing coding agents')
  })

  it('discovering and connecting: indeterminate, with the step', () => {
    const discovering = desktopSync(desktop({ run: local('discovering', importingAgents(), NOW) }))
    expect(discovering.value).toBeNull()
    expect(syncLine(discovering)).toBe('Syncing coding agents · Looking for agents')
    const connecting = desktopSync(desktop({ run: local('connecting', importingAgents(), NOW) }))
    expect(syncLine(connecting)).toBe('Syncing coding agents · Connecting')
  })

  it('a desktop without a queue report counts the run’s own waiting turns', () => {
    const agents = doneAgents().map((a) =>
      a.id === 'claude' ? { ...a, import: { ...a.import, pending: 7 } } : a,
    )
    const sync = desktopSync(desktop({ run: local('done', agents, NOW), backlog: undefined }))
    expect(sync).toMatchObject({ syncing: true, phase: 'sending', waiting: 7, sent: 835 })
  })
})

describe('desktopPollInterval', () => {
  it('every second while syncing, every 10 s while idle, never in a browser', () => {
    expect(desktopPollInterval(null)).toBe(false)
    expect(desktopPollInterval(undefined)).toBe(false)
    expect(desktopPollInterval(desktop())).toBe(DESKTOP_IDLE_POLL_MS)
    expect(desktopPollInterval(desktop({ backlog: queue(3) }))).toBe(DESKTOP_STATE_POLL_MS)
  })
})
