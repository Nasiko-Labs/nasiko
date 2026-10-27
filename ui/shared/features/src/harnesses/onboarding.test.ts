/** The onboarding card's pure rules (runs.ts, onboarding.ts): its state per mode, when it shows, when it polls, and the
 *  run math (progress, sending, windows, refused turns). */
import { describe, expect, it } from 'vitest'
import {
  doneAgents,
  importingAgents,
  mockRun,
  mockWindows,
  rejectedAgents,
} from '@nasiko/mocks/codingAgents'
import { SYNC_IDLE_POLL_MS, SYNC_POLL_MS, SYNC_SENDING_POLL_MS } from './constants'
import { desktopStateSchema, type DesktopState } from './desktop'
import { browserView, cardVisible, desktopView, pollInterval } from './onboarding'
import {
  awaitingRetry,
  lastMonthIn,
  pendingTurns,
  rejectedTurns,
  runProgress,
  runTotals,
  shownPhase,
  syncRunsSchema,
  type SyncRun,
} from './runs'

const NOW = Date.parse('2026-03-20T15:00:00Z')
const run = (...args: Parameters<typeof mockRun>): SyncRun => mockRun(...args)
const iso = (ms: number) => new Date(ms).toISOString()
/** The finished agents with `n` of Claude Code's turns still queued. */
const pendingAgents = (n: number) =>
  doneAgents().map((a) => (a.id === 'claude' ? { ...a, import: { ...a.import, pending: n } } : a))
const desktop = (over: Partial<DesktopState> = {}): DesktopState => ({
  desktop: true,
  consent: 'granted',
  signed_in: true,
  run: null,
  ...over,
})

describe('browserView', () => {
  it('shows the command with no run, then the runs', () => {
    expect(browserView([])).toEqual({ kind: 'command' })
    // A v1 client's consent wait is never shown.
    expect(browserView([run('awaiting_consent', [], NOW)])).toEqual({ kind: 'command' })
    expect(browserView([run('connecting', [], NOW)]).kind).toBe('running')
    expect(browserView([run('done', doneAgents(), NOW)]).kind).toBe('finished')
    expect(browserView([run('failed', [], NOW)]).kind).toBe('finished')
  })

  it('an active run on one device wins over a finished one on another', () => {
    const v = browserView([
      run('importing', importingAgents(), NOW, { device_id: 'a' }),
      run('done', doneAgents(), NOW, { device_id: 'b' }),
    ])
    expect(v.kind).toBe('running')
    expect(v.kind === 'running' && v.runs.map((r) => r.device_id)).toEqual(['a', 'b'])
  })
})

describe('desktopView', () => {
  it('asks while unasked, then a quiet line once declined', () => {
    expect(desktopView(desktop({ consent: 'unasked' }))).toEqual({ kind: 'ask' })
    expect(desktopView(desktop({ consent: 'declined' }))).toEqual({ kind: 'declined' })
    expect(desktopView(desktop())).toEqual({ kind: 'idle' })
  })

  it("shows the desktop's own run as this computer's", () => {
    const {
      device_id: _d,
      device_name: _n,
      client: _c,
      ...local
    } = run('importing', importingAgents(), NOW)
    const v = desktopView(desktop({ run: local }))
    expect(v.kind).toBe('running')
    expect(v.kind === 'running' && v.runs[0].device_name).toBe('This computer')
  })

  it('only a 200 JSON body with desktop: true is the desktop', () => {
    expect(desktopStateSchema.safeParse(desktop()).success).toBe(true)
    expect(desktopStateSchema.safeParse({ ...desktop(), desktop: false }).success).toBe(false)
    expect(desktopStateSchema.safeParse('<!doctype html>').success).toBe(false)
  })
})

describe('cardVisible and pollInterval', () => {
  it('the ask and an active run always show; a browser shows any run, the command only without harnesses', () => {
    expect(cardVisible({ kind: 'ask' }, 'desktop', false, false)).toBe(true)
    expect(cardVisible({ kind: 'running', runs: [] }, 'browser', false, false)).toBe(true)
    expect(cardVisible({ kind: 'finished', runs: [] }, 'browser', false, false)).toBe(true)
    expect(cardVisible({ kind: 'command' }, 'browser', false, false)).toBe(false)
    expect(cardVisible({ kind: 'command' }, 'browser', true, false)).toBe(true)
  })

  it('desktop: a finished, declined or idle card only without harnesses, after taking part, or needing attention', () => {
    for (const v of [
      { kind: 'declined' },
      { kind: 'idle' },
      { kind: 'finished', runs: [] },
    ] as const) {
      expect(cardVisible(v, 'desktop', false, false)).toBe(false)
      expect(cardVisible(v, 'desktop', true, false)).toBe(true)
      expect(cardVisible(v, 'desktop', false, true)).toBe(true)
    }
    const rejected = run('done', rejectedAgents(), NOW)
    expect(cardVisible({ kind: 'finished', runs: [rejected] }, 'desktop', false, false)).toBe(true)
  })

  it('polls fast while reading history, slowly while idle or sending, never once finished', () => {
    const importing = run('importing', importingAgents(), NOW)
    expect(pollInterval({ kind: 'running', runs: [importing] })).toBe(SYNC_POLL_MS)
    expect(pollInterval({ kind: 'command' })).toBe(SYNC_IDLE_POLL_MS)
    const sending = run('sending', pendingAgents(5), NOW)
    expect(pollInterval({ kind: 'running', runs: [sending] })).toBe(SYNC_SENDING_POLL_MS)
    expect(pollInterval({ kind: 'finished', runs: [run('done', doneAgents(), NOW)] })).toBe(false)
  })

  it('keeps polling while a Retry waits for the client, and stops once it reports', () => {
    const asked = run('done', rejectedAgents(), NOW, { retry_requested_at: iso(NOW) })
    expect(pollInterval({ kind: 'finished', runs: [asked] })).toBe(SYNC_SENDING_POLL_MS)
    const picked = run('done', rejectedAgents(), NOW + 1000, { retry_requested_at: iso(NOW) })
    expect(pollInterval({ kind: 'finished', runs: [picked] })).toBe(false)
  })
})

describe('runProgress and runTotals', () => {
  it('is unknown while discovering or connecting, or with nothing counted', () => {
    expect(runProgress(run('discovering', importingAgents(), NOW))).toBeNull()
    expect(runProgress(run('connecting', importingAgents(), NOW))).toBeNull()
    expect(runProgress(run('importing', [], NOW))).toBeNull()
  })

  it('sums done over total across agents, and a done run is full', () => {
    expect(runProgress(run('importing', importingAgents(), NOW))).toBe(37)
    expect(runProgress(run('done', [], NOW))).toBe(100)
  })

  it('totals only the agents that imported', () => {
    const t = runTotals(run('done', doneAgents(), NOW))
    expect([t.sessions, t.turns, t.agents.map((a) => a.id)]).toEqual([
      115,
      842,
      ['claude', 'codex', 'opencode'],
    ])
  })
})

describe('v2: sending, windows, refused turns', () => {
  it('a done run with turns still queued shows as sending, never done', () => {
    const r = run('done', pendingAgents(7), NOW)
    expect(pendingTurns(r)).toBe(7)
    expect(shownPhase(r)).toBe('sending')
    expect(browserView([r]).kind).toBe('running')
    expect(runProgress(r)).not.toBe(100)
    expect(shownPhase(run('done', doneAgents(), NOW))).toBe('done')
  })

  it('progress is windows done over windows once the client reports them', () => {
    // 10 of the 31 windows done.
    const r = run('importing', importingAgents(), NOW, { windows: mockWindows(NOW, 10) })
    expect(runProgress(r)).toBe(32)
  })

  it('the last month is in once every window but "Older" is done', () => {
    expect(lastMonthIn(mockWindows(NOW, 30))).toBe(true)
    expect(lastMonthIn(mockWindows(NOW, 29))).toBe(false)
    expect(lastMonthIn(mockWindows(NOW, 31))).toBe(false)
    expect(lastMonthIn([])).toBe(false)
  })

  it('counts refused turns, and a Retry waits until the client reports after it', () => {
    expect(rejectedTurns(run('done', rejectedAgents(), NOW))).toBe(3)
    expect(awaitingRetry(run('done', rejectedAgents(), NOW))).toBe(false)
    expect(awaitingRetry(run('done', [], NOW, { retry_requested_at: iso(NOW) }))).toBe(true)
    expect(awaitingRetry(run('done', [], NOW + 1, { retry_requested_at: iso(NOW) }))).toBe(false)
  })

  it('a v1 run (no windows, pending, rejected or retry fields) parses with the defaults', () => {
    const v1 = {
      device_id: 'd',
      device_name: 'Old Mac',
      client: 'cli',
      phase: 'importing',
      agents: [
        {
          id: 'claude',
          name: 'Claude Code',
          detected: true,
          connected: true,
          import: { status: 'running', done: 1, total: 4, sessions: 1, turns: 3, failed: 0 },
        },
      ],
      error: null,
      started_at: null,
      updated_at: iso(NOW),
    }
    const parsed = syncRunsSchema.parse({ runs: [v1] }).runs[0]
    expect(parsed.windows).toEqual([])
    expect(parsed.retry_requested_at).toBeNull()
    expect(parsed.agents[0].import).toMatchObject({ pending: 0, rejected: 0, rejected_reasons: [] })
    expect(runProgress(parsed)).toBe(25)
  })
})

it('the mock runs parse with the wire schema', () => {
  const runs = [
    run('importing', importingAgents(), NOW, { windows: mockWindows(NOW, 3) }),
    run('done', rejectedAgents(), NOW, { retry_requested_at: iso(NOW) }),
  ]
  expect(syncRunsSchema.safeParse({ runs }).success).toBe(true)
})
