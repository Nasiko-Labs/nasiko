/**
 * The lazy half of error reporting: the Sentry SDK and its setup, one chunk loaded after first paint and only when the
 * server says reporting is on, so it costs the shell nothing.
 */
import * as Sentry from '@sentry/react'
// The replay compression worker as a same-origin file (Vite emits it under /assets). The SDK's default worker is a
// blob: URL, which the CSP refuses (worker-src falls back to script-src 'self'); without compression a replay is
// sent raw, far over the tunnel's 2 MiB cap, and the server answers 413.
import replayWorkerUrl from '@sentry/replay/worker-bundler?url'
import type { QueryClient } from '@tanstack/react-query'
import type { AnyRouter } from '@tanstack/react-router'
import { meQuery, type Me } from '@nasiko/data-access/auth'
import { apiUrl } from '@nasiko/data-access/client'
import { uiClient, uiRelease } from '@nasiko/app-core/env'
import { attachSink } from './queue'
import { scrubBreadcrumb, scrubEvent, scrubLog, scrubSpan } from './scrub'
import type { ErrorReportingSettings } from './settings'

export function startSentry(settings: ErrorReportingSettings, router: AnyRouter, edition: string) {
  const api = apiUrl('/api/')
  const sameOrigin = api.origin === globalThis.location?.origin
  const replay = settings.replaysSessionSampleRate > 0 || settings.replaysOnErrorSampleRate > 0
  Sentry.init({
    dsn: settings.dsn,
    // Same-origin through the server's tunnel (its `error_reporting` module): the CSP stays connect-src 'self',
    // and ad blockers, which block *.sentry.io, don't drop the report.
    tunnel: apiUrl(settings.tunnel).href,
    environment: settings.environment,
    release: uiRelease ?? undefined,
    // Collect as little as possible at the source; `scrub.ts` then cleans what is left. Every default here is "collect".
    dataCollection: {
      userInfo: false,
      cookies: false,
      httpHeaders: { request: { allow: ['user-agent'] }, response: false },
      httpBodies: [],
      urlQueryParams: false,
      stackFrameVariables: false,
    },
    integrations: [
      Sentry.tanstackRouterBrowserTracingIntegration(router),
      ...(replay
        ? [
            // Replays show layout and clicks, never content: every text node, input and image is masked or blocked.
            Sentry.replayIntegration({
              workerUrl: replayWorkerUrl,
              maskAllText: true,
              maskAllInputs: true,
              blockAllMedia: true,
              networkDetailAllowUrls: [],
            }),
          ]
        : []),
      // Warnings and errors from the console as Sentry Logs, linked to the trace and the replay; `scrubLog` cleans them.
      ...(settings.logs ? [Sentry.consoleLoggingIntegration({ levels: ['warn', 'error'] })] : []),
    ],
    tracesSampleRate: settings.tracesSampleRate,
    // Trace headers go to our own API only, and only same-origin: a cross-origin API base (an edition that called
    // setApiBase) would need sentry-trace and baggage in its CORS allow-list, and a refused preflight fails the call.
    tracePropagationTargets: sameOrigin ? [api.href] : [],
    replaysSessionSampleRate: settings.replaysSessionSampleRate,
    replaysOnErrorSampleRate: settings.replaysOnErrorSampleRate,
    beforeSend: scrubEvent,
    beforeSendSpan: scrubSpan,
    beforeBreadcrumb: scrubBreadcrumb,
    beforeSendLog: scrubLog,
  })
  Sentry.setTag('edition', edition)
  Sentry.setTag('client', uiClient)
  if (settings.workspace) Sentry.setTag('workspace', settings.workspace)
  followSignedInUser(
    (router.options.context as { queryClient?: QueryClient } | undefined)?.queryClient,
  )
  attachSink(({ error, componentStack, handled }) => {
    const mechanism = { handled, type: componentStack ? 'react.root' : 'onerror' }
    if (componentStack) Sentry.captureReactException(error, { componentStack }, { mechanism })
    else Sentry.captureException(error, { mechanism })
  })
}

/**
 * Name the signed-in user on every report by account id only (never the name or email), so Sentry can count how many
 * people an error reaches. Follows the session query: signing in sets it, signing out clears it.
 */
export function followSignedInUser(queryClient: QueryClient | undefined) {
  if (!queryClient) return
  const key = JSON.stringify(meQuery.queryKey)
  const apply = () => {
    const me = queryClient.getQueryData<Me>(meQuery.queryKey)
    Sentry.setUser(me?.sub ? { id: me.sub } : null)
  }
  apply()
  queryClient.getQueryCache().subscribe((event) => {
    if (JSON.stringify(event.query.queryKey) === key) apply()
  })
}
