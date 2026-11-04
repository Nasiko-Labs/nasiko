import type { AnyRouter } from '@tanstack/react-router'
import { env } from '@nasiko/app-core/env'
import { stopReporting } from './queue'
import { loadErrorReportingSettings } from './settings'

/** After first paint the browser is idle within a frame or two; the deadline covers a page that never goes idle. */
const IDLE_DEADLINE_MS = 3000

/**
 * Ask the server whether to report, and if so load and start the SDK, once the page is idle. Never in the dev server
 * or in mock mode: the data there is fabricated, and so would be the reports.
 */
export function startErrorReporting(router: AnyRouter, edition: string) {
  if (import.meta.env.DEV || env.mode === 'mock') {
    stopReporting()
    return
  }
  whenIdle(() => void start(router, edition))
}

async function start(router: AnyRouter, edition: string) {
  try {
    const settings = await loadErrorReportingSettings()
    if (!settings) return stopReporting()
    const { startSentry } = await import('./sentry')
    startSentry(settings, router, edition)
  } catch {
    // An older server without the endpoint, a network blip, or a chunk that won't load: the app works the same,
    // it just reports nothing this load.
    stopReporting()
  }
}

function whenIdle(task: () => void) {
  if (typeof globalThis.requestIdleCallback === 'function')
    globalThis.requestIdleCallback(task, { timeout: IDLE_DEADLINE_MS })
  else setTimeout(task, 0)
}
