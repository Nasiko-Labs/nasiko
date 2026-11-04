/**
 * The lazy half of error reporting: the Sentry SDK and its setup, one chunk loaded after first paint and only when the
 * server says reporting is on, so it costs the shell nothing.
 */
import * as Sentry from '@sentry/react'
import type { AnyRouter } from '@tanstack/react-router'
import { apiUrl } from '@nasiko/data-access/client'
import { uiRelease } from '@nasiko/app-core/env'
import { attachSink } from './queue'
import { scrubBreadcrumb, scrubEvent, scrubTransaction } from './scrub'
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
              maskAllText: true,
              maskAllInputs: true,
              blockAllMedia: true,
              networkDetailAllowUrls: [],
            }),
          ]
        : []),
    ],
    tracesSampleRate: settings.tracesSampleRate,
    // Trace headers go to our own API only, and only same-origin: a cross-origin API base (an edition that called
    // setApiBase) would need sentry-trace and baggage in its CORS allow-list, and a refused preflight fails the call.
    tracePropagationTargets: sameOrigin ? [api.href] : [],
    replaysSessionSampleRate: settings.replaysSessionSampleRate,
    replaysOnErrorSampleRate: settings.replaysOnErrorSampleRate,
    beforeSend: scrubEvent,
    beforeSendTransaction: scrubTransaction,
    beforeBreadcrumb: scrubBreadcrumb,
  })
  Sentry.setTag('edition', edition)
  attachSink(({ error, componentStack, handled }) => {
    const mechanism = { handled, type: componentStack ? 'react.root' : 'onerror' }
    if (componentStack) Sentry.captureReactException(error, { componentStack }, { mechanism })
    else Sentry.captureException(error, { mechanism })
  })
}
