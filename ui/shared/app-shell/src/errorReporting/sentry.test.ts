import { QueryClient } from '@tanstack/react-query'
import type { AnyRouter } from '@tanstack/react-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { meQuery } from '@nasiko/data-access/auth'
import type { ErrorReportingSettings } from './settings'

const setUser = vi.fn()
const sdk = vi.hoisted(() => ({
  init: vi.fn(),
  setTag: vi.fn(),
  captureException: vi.fn(),
  captureReactException: vi.fn(),
  tanstackRouterBrowserTracingIntegration: vi.fn(() => ({ name: 'router' })),
  consoleLoggingIntegration: vi.fn(() => ({ name: 'console' })),
  replayIntegration: vi.fn((_options: Record<string, unknown>) => ({ name: 'replay' })),
}))
vi.mock('@sentry/react', () => ({ ...sdk, setUser: (user: unknown) => setUser(user) }))
vi.mock('@sentry/replay/worker-bundler?url', () => ({ default: '' }))

const { followSignedInUser, startSentry } = await import('./sentry')

const settings = (replays: Partial<ErrorReportingSettings>): ErrorReportingSettings => ({
  dsn: 'https://key@o0.ingest.sentry.io/1',
  environment: 'test',
  tunnel: '/api/telemetry/sentry',
  tracesSampleRate: 0,
  replaysSessionSampleRate: 0,
  replaysOnErrorSampleRate: 0,
  logs: false,
  workspace: null,
  ...replays,
})
const router = { options: { context: {} } } as unknown as AnyRouter

describe('session replay', () => {
  beforeEach(() => sdk.replayIntegration.mockClear())

  it.each([
    ['a session rate', { replaysSessionSampleRate: 0.1 }],
    ['an on-error rate', { replaysOnErrorSampleRate: 1 }],
  ])(
    'session_replay_masks_all_text_and_inputs, with %s',
    (_name, rates: Partial<ErrorReportingSettings>) => {
      startSentry(settings(rates), router, 'oss')
      expect(sdk.replayIntegration).toHaveBeenCalledTimes(1)
      expect(sdk.replayIntegration.mock.calls[0]![0]).toMatchObject({
        maskAllText: true,
        maskAllInputs: true,
        blockAllMedia: true,
      })
    },
  )

  it('is not started at all when both rates are zero', () => {
    startSentry(settings({}), router, 'oss')
    expect(sdk.replayIntegration).not.toHaveBeenCalled()
  })
})

describe('followSignedInUser', () => {
  it('names the signed-in user by id only, follows sign-in and sign-out, and does nothing without a client', () => {
    followSignedInUser(undefined)
    expect(setUser).not.toHaveBeenCalled()

    const client = new QueryClient()
    followSignedInUser(client)
    expect(setUser).toHaveBeenLastCalledWith(null)

    client.setQueryData(meQuery.queryKey, { sub: 'u-123', username: 'ada', is_superuser: false })
    expect(setUser).toHaveBeenLastCalledWith({ id: 'u-123' })

    client.removeQueries({ queryKey: meQuery.queryKey })
    expect(setUser).toHaveBeenLastCalledWith(null)
  })
})
