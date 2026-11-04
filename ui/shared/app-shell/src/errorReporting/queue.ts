/**
 * The eager half of error reporting: it is in the shell, so it stays tiny. It holds what goes wrong between the first
 * script and the moment the Sentry SDK (a lazy chunk, `./sentry.ts`) is running, then hands that over.
 *
 * Flutter comparison: `runZonedGuarded` + `FlutterError.onError` set before `runApp`, buffering until Crashlytics is
 * ready. The web has no zone: the page-wide `error` and `unhandledrejection` events are the catch-all, and React's own
 * root hooks report what an error boundary caught.
 */

export interface CapturedError {
  error: unknown
  /** React's component stack, for an error a component threw. */
  componentStack?: string
  /** True when an error boundary showed a fallback for it (the page survived). */
  handled: boolean
}

type Sink = (captured: CapturedError) => void

/** Enough to explain a broken first load; an endless error loop must not grow memory. */
const MAX_PENDING = 20

let pending: CapturedError[] = []
let sink: Sink | null = null
let stopped = false
let removeListeners: (() => void) | null = null

/** Hand a failure to the SDK, or hold it until the SDK starts. */
export function report(captured: CapturedError) {
  if (sink) sink(captured)
  else if (!stopped && pending.length < MAX_PENDING) pending.push(captured)
}

/**
 * Listen for page-wide errors from the first script on. The SDK installs the same listeners when it starts
 * (`attachSink` removes these then), so nothing is reported twice.
 */
export function captureEarlyErrors(target: Window = window) {
  if (removeListeners) return
  const onError = (e: ErrorEvent) => report({ error: e.error ?? e.message, handled: false })
  const onRejection = (e: PromiseRejectionEvent) => report({ error: e.reason, handled: false })
  target.addEventListener('error', onError)
  target.addEventListener('unhandledrejection', onRejection)
  removeListeners = () => {
    target.removeEventListener('error', onError)
    target.removeEventListener('unhandledrejection', onRejection)
    removeListeners = null
  }
}

/** The SDK is running: replay what was held, then report straight through. */
export function attachSink(next: Sink) {
  removeListeners?.()
  sink = next
  const held = pending
  pending = []
  held.forEach(next)
}

/** Reporting is off for this deployment (or failed to start): drop what was held and stop holding. */
export function stopReporting() {
  removeListeners?.()
  stopped = true
  pending = []
}

/** React 19 root options: what a component threw, logged as React would and then reported. */
export const reactRootErrorHandlers = {
  onCaughtError(error: unknown, info: { componentStack?: string }) {
    console.error(error)
    report({ error, componentStack: info.componentStack, handled: true })
  },
  onUncaughtError(error: unknown, info: { componentStack?: string }) {
    console.error(error)
    report({ error, componentStack: info.componentStack, handled: false })
  },
}

/** Tests only: back to the state before the first script. */
export function resetForTests() {
  removeListeners?.()
  pending = []
  sink = null
  stopped = false
}
