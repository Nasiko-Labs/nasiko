import '@testing-library/jest-dom/vitest'
import { cleanup, configure } from '@testing-library/react'
import { MotionGlobalConfig } from 'motion/react'
import { setupServer } from 'msw/node'
import { afterAll, afterEach, beforeAll, beforeEach } from 'vitest'
import { allHandlers, resetAgentsMock } from '@/mocks/handlers'
import { editionHandlers } from '@edition/mocks'

/**
 * A working, per-test localStorage. Node 25's built-in global shadows jsdom's and throws on use
 * (it needs --localstorage-file), which silently turned every storage read into "unavailable" and
 * let state leak between tests. Each test starts with an empty store.
 */
function memoryStorage(): Storage {
  const m = new Map<string, string>()
  return {
    get length() {
      return m.size
    },
    clear: () => m.clear(),
    getItem: (k) => m.get(k) ?? null,
    key: (i) => [...m.keys()][i] ?? null,
    removeItem: (k) => void m.delete(k),
    setItem: (k, v) => void m.set(k, String(v)),
  }
}
// jsdom has no pointer-capture API; Radix Select calls it when it opens. Node-environment test
// files (`@vitest-environment node`) have no Element.
if (typeof Element !== 'undefined') {
  const proto = Element.prototype as unknown as Record<string, unknown>
  for (const k of ['hasPointerCapture', 'releasePointerCapture', 'setPointerCapture']) {
    if (!(k in proto)) proto[k] = k === 'hasPointerCapture' ? () => false : () => {}
  }
}

beforeEach(() => {
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    writable: true,
    value: memoryStorage(),
  })
})

// Animations finish instantly in tests: assertions never wait on a spring.
MotionGlobalConfig.skipAnimations = true

// jsdom has no layout: visx's ParentSize and Radix need ResizeObserver to exist.
class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver
// jsdom has no scrollIntoView; cmdk (the chat TargetPicker's list) calls it on the selected item.
if (typeof Element !== 'undefined')
  Element.prototype.scrollIntoView ??= function scrollIntoView() {}

// Full-page renders under a parallel run (or coverage instrumentation) can exceed the 1 s
// findBy default on a busy machine, and a file's first render also pays for the React Compiler's
// Babel pass over the page's modules; 12 s keeps page tests deterministic without masking hangs.
configure({ asyncUtilTimeout: 12_000 })

/** Every test runs against the same seed-derived handlers as the browser (plan A22), the edition's own first. */
export const server = setupServer(...Object.values(editionHandlers).flat(), ...allHandlers)

/** Record the URL of every request the page makes; call `stop()` in a finally block. */
export function recordRequests() {
  const urls: URL[] = []
  const onStart = ({ request }: { request: Request }) => {
    urls.push(new URL(request.url))
  }
  server.events.on('request:start', onStart)
  return { urls, stop: () => server.events.removeListener('request:start', onStart) }
}

export interface RecordedBody {
  method: string
  url: URL
  /** Parsed JSON, the raw text when it isn't JSON, or null with no body. */
  body: unknown
}

/**
 * Record every request's method, URL and parsed body (v1b EN-11). Each request is cloned before
 * reading, so handlers still see the original body. Assert only after `await flush()`, which
 * waits for pending body reads; `parseErrors` counts bodies that couldn't be read at all.
 */
export function recordRequestBodies() {
  const requests: RecordedBody[] = []
  const pending = new Set<Promise<void>>()
  let parseErrors = 0
  const onStart = ({ request }: { request: Request }) => {
    const clone = request.clone()
    const p = clone
      .text()
      .then(
        (raw) => {
          let body: unknown = raw || null
          try {
            body = raw ? JSON.parse(raw) : null
          } catch {
            /* not JSON: keep the text */
          }
          requests.push({ method: request.method, url: new URL(request.url), body })
        },
        () => {
          parseErrors++
        },
      )
      .finally(() => pending.delete(p))
    pending.add(p)
  }
  server.events.on('request:start', onStart)
  return {
    requests,
    get parseErrors() {
      return parseErrors
    },
    async flush() {
      while (pending.size) await Promise.all([...pending])
    },
    /**
     * POSTs that would save an assistant row: the client must never send one on a routed chat.
     * Fails closed: an unreadable body could have been one.
     */
    assistantPosts: () => {
      if (parseErrors) throw new Error(`${parseErrors} request bodies couldn't be read`)
      return requests.filter(
        (r) =>
          r.method === 'POST' &&
          /\/api\/chat\/sessions\/[^/]+\/messages$/.test(r.url.pathname) &&
          (r.body as { role?: unknown } | null)?.role === 'assistant',
      )
    },
    stop: () => server.events.removeListener('request:start', onStart),
  }
}

beforeAll(() => server.listen({ onUnhandledRequest: 'error' }))
afterEach(async () => {
  cleanup()
  server.resetHandlers()
  resetAgentsMock()
  // Chat's per-user state (turns, signals, the request index, the poll backoff) never leaks between tests (DX3).
  // Imported lazily, so a test file's vi.mock of a chat module still applies to it (and one that fails to load is skipped).
  await import('@/features/chat/registry').then(
    (m) => m.clearChatRegistry(),
    () => undefined,
  )
})
afterAll(() => server.close())
