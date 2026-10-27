// The stale-build recovery rules (./staleBuild.ts). Ported from codex/nasiko-react-foundation
// ui/shared/app-core/src/stale-build.test.ts.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  RELOAD_KEY,
  RELOAD_WINDOW_MS,
  STALE_BUILD_EVENT,
  checkVersion,
  hasUnsavedEdits,
  installStaleBuildDetection,
  isChunkLoadError,
  recoverFromChunkError,
  recoverFromStaleRoute,
  registerUnsavedEdits,
  resetStaleBuildForTests,
} from './staleBuild'
import { BOOT_GUARD_SCRIPT } from './staleBuildBoot'

describe('stale build recovery', () => {
  beforeEach(() => {
    resetStaleBuildForTests()
    window.sessionStorage.clear()
  })
  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('recognizes the chunk-load errors of each engine, also as a cause', () => {
    for (const message of [
      'Failed to fetch dynamically imported module: http://x/assets/a.js',
      'error loading dynamically imported module: http://x/assets/a.js',
      'Importing a module script failed.',
      'Unable to preload CSS for /assets/a.css',
    ])
      expect(isChunkLoadError(new TypeError(message))).toBe(true)
    expect(
      isChunkLoadError(
        new Error('wrapped', { cause: new TypeError('Importing a module script failed.') }),
      ),
    ).toBe(true)
    expect(isChunkLoadError(new Error('Cannot read properties of undefined'))).toBe(false)
    expect(isChunkLoadError(null)).toBe(false)
  })

  it('reloads once onto the same URL when nothing is unsaved, and records when', () => {
    const reload = vi.fn()
    expect(recoverFromStaleRoute(reload, 1_000_000)).toBe('reloading')
    expect(reload).toHaveBeenCalledTimes(1)
    expect(window.sessionStorage.getItem(RELOAD_KEY)).toBe('1000000')
  })

  it('does not reload over unsaved edits, and reloads again once they are gone', () => {
    const reload = vi.fn()
    const release = registerUnsavedEdits()
    expect(hasUnsavedEdits()).toBe(true)
    expect(recoverFromStaleRoute(reload)).toBe('held')
    expect(reload).not.toHaveBeenCalled()
    release()
    expect(hasUnsavedEdits()).toBe(false)
    expect(recoverFromStaleRoute(reload)).toBe('reloading')
  })

  it('a reload under way is not reported as failed by a second boundary', () => {
    const reload = vi.fn()
    expect(recoverFromStaleRoute(reload, 10_000)).toBe('reloading')
    expect(recoverFromStaleRoute(reload, 10_001)).toBe('reloading')
    expect(reload).toHaveBeenCalledTimes(1)
  })

  it('allows an upgrade and a quick rollback, then gives up instead of looping', () => {
    const reload = vi.fn()
    // One automatic reload already in the window: a second real transition (a rollback) still recovers.
    window.sessionStorage.setItem(RELOAD_KEY, String(5_000))
    expect(recoverFromStaleRoute(reload, 6_000)).toBe('reloading')
    expect(window.sessionStorage.getItem(RELOAD_KEY)).toBe('5000,6000')
    // Two in the window: the next failure stops (a flapping rollout or a server really missing a chunk).
    resetStaleBuildForTests()
    expect(recoverFromStaleRoute(reload, 7_000)).toBe('exhausted')
    expect(reload).toHaveBeenCalledTimes(1)
    // Once the window has passed, recovery works again.
    resetStaleBuildForTests()
    expect(recoverFromStaleRoute(reload, 6_000 + RELOAD_WINDOW_MS + 1)).toBe('reloading')
  })

  it('does not reload when storage is unavailable (a loop could not be ruled out)', () => {
    // The setup installs a fresh in-memory storage per test (shared/testing/src/setup.ts), so block that one.
    vi.spyOn(window.sessionStorage, 'getItem').mockImplementation(() => {
      throw new Error('SecurityError')
    })
    const reload = vi.fn()
    expect(recoverFromStaleRoute(reload)).toBe('exhausted')
    expect(reload).not.toHaveBeenCalled()
  })

  it('a failed preload marks the build stale only when the version changed, and never reloads', async () => {
    const stale = vi.fn()
    window.addEventListener(STALE_BUILD_EVENT, stale)
    const offlineTarget = new EventTarget() as unknown as Window
    installStaleBuildDetection(offlineTarget, () => Promise.resolve('offline'))
    offlineTarget.dispatchEvent(new Event('vite:preloadError'))
    await vi.waitFor(() => Promise.resolve())
    expect(stale).not.toHaveBeenCalled()
    const target = new EventTarget() as unknown as Window
    installStaleBuildDetection(target, () => Promise.resolve('changed'))
    target.dispatchEvent(new Event('vite:preloadError'))
    await vi.waitFor(() => {
      expect(stale).toHaveBeenCalledTimes(1)
    })
    // No reload was attempted: an automatic reload always records its time first.
    expect(window.sessionStorage.getItem(RELOAD_KEY)).toBeNull()
    window.removeEventListener(STALE_BUILD_EVENT, stale)
  })

  it('the boot guard shares the marker and limits with the in-app recovery', () => {
    expect(BOOT_GUARD_SCRIPT).toContain(JSON.stringify(RELOAD_KEY))
    expect(BOOT_GUARD_SCRIPT).toContain(`WINDOW = ${String(RELOAD_WINDOW_MS)}`)
  })

  describe('version check', () => {
    const html = (entry: string) =>
      Promise.resolve(new Response(`<html><script type="module" src="${entry}"></script></html>`))
    beforeEach(() => {
      document.head.innerHTML = '<script type="module" src="/assets/index-old.js"></script>'
    })

    it('tells a new build from the same one', async () => {
      expect(
        await checkVersion(
          () => html('/assets/index-new.js'),
          () => true,
        ),
      ).toBe('changed')
      expect(
        await checkVersion(
          () => html('/assets/index-old.js'),
          () => true,
        ),
      ).toBe('same')
    })

    it("reads the shell from the page's origin, never cached", async () => {
      const fetchSpy = vi.fn((_url: string, _init: RequestInit) => html('/assets/index-old.js'))
      await checkVersion(fetchSpy, () => true)
      expect(fetchSpy).toHaveBeenCalledWith(
        new URL('/', location.origin).href,
        expect.objectContaining({ cache: 'no-store' }),
      )
    })

    it('tells offline and an unreachable server from a new version', async () => {
      const fetchSpy = vi.fn(() => html('/assets/index-new.js'))
      expect(await checkVersion(fetchSpy, () => false)).toBe('offline')
      expect(fetchSpy).not.toHaveBeenCalled()
      expect(
        await checkVersion(
          () => Promise.reject(new TypeError('fetch failed')),
          () => true,
        ),
      ).toBe('unreachable')
      expect(
        await checkVersion(
          () => Promise.resolve(new Response('', { status: 502 })),
          () => true,
        ),
      ).toBe('unreachable')
    })
  })

  describe('chunk error recovery', () => {
    it('reloads only for a changed version', async () => {
      const reload = vi.fn()
      expect(await recoverFromChunkError({ check: () => Promise.resolve('same'), reload })).toBe(
        'failed',
      )
      expect(
        await recoverFromChunkError({ check: () => Promise.resolve('unreachable'), reload }),
      ).toBe('network')
      expect(reload).not.toHaveBeenCalled()
      expect(await recoverFromChunkError({ check: () => Promise.resolve('changed'), reload })).toBe(
        'reloading',
      )
      expect(reload).toHaveBeenCalledTimes(1)
    })

    it('offline: waits, then reloads once when the connection is back (unless edits are unsaved)', async () => {
      const target = new EventTarget() as unknown as Window
      const reload = vi.fn()
      expect(
        await recoverFromChunkError({ check: () => Promise.resolve('offline'), reload, target }),
      ).toBe('offline')
      expect(reload).not.toHaveBeenCalled()
      target.dispatchEvent(new Event('online'))
      expect(reload).toHaveBeenCalledTimes(1)

      resetStaleBuildForTests()
      window.sessionStorage.clear()
      const release = registerUnsavedEdits()
      const target2 = new EventTarget() as unknown as Window
      await recoverFromChunkError({
        check: () => Promise.resolve('offline'),
        reload,
        target: target2,
      })
      target2.dispatchEvent(new Event('online'))
      expect(reload).toHaveBeenCalledTimes(1)
      release()
    })
  })
})
