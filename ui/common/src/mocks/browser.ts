/**
 * Starts the MSW service worker in the browser. Loaded only via dynamic import from
 * main.tsx, so none of this ships in a live-mode production bundle (plan A22).
 */
import { setupWorker } from 'msw/browser'
import type { EnvConfig } from '@/lib/env'
import { readAnchor, pinClock } from './anchor'
import { editionHandlers } from '@edition/mocks'
import { allHandlers, configureMocks, handlersFor, type MockOnboarding } from './handlers'
import { generateSeed } from './seed'

/** Mock-only key: the mock session was ended by a sign-out. */
const MOCK_SIGNED_OUT_KEY = 'ui-lab:mock-signed-out'

/** Storage blocked: null, so every load starts signed in, as before. */
function readMockSignedOut(): boolean | null {
  try {
    return localStorage.getItem(MOCK_SIGNED_OUT_KEY) === '1'
  } catch {
    return null
  }
}

/** Mock-only key: the mock user's onboarding row, so a finished guide stays finished across reloads. */
const MOCK_ONBOARDING_KEY = 'ui-lab:mock-onboarding'

/** A first-time user unless a finished guide was stored; storage blocked: first-time on every load. */
function readMockOnboarding(): MockOnboarding {
  try {
    const raw = localStorage.getItem(MOCK_ONBOARDING_KEY)
    if (raw) return JSON.parse(raw) as MockOnboarding
  } catch {
    // Unreadable: start over.
  }
  return { persona: null, completed: false }
}

export async function startMocks(cfg: EnvConfig): Promise<void> {
  // The edition's own mocks answer first, each as its own server would; this module is reached only by the mock-mode
  // dynamic import, so they never ship in a live bundle.
  const handlers =
    cfg.mode === 'mock'
      ? [...Object.values(editionHandlers).flat(), ...allHandlers]
      : handlersFor(cfg.partialMocks, editionHandlers)
  if (!handlers.length) return
  // Pinned demo time: only in full mock mode (a live server's data has its own clock).
  const anchor =
    cfg.mode === 'mock'
      ? readAnchor(location.search, import.meta.env.VITE_NASIKO_SEED_ANCHOR)
      : null
  if (anchor) {
    pinClock(anchor)
    configureMocks({ seed: generateSeed({ anchor }), now: () => Date.now() })
    console.info(`[ui-lab] demo time pinned to ${anchor.toISOString()} (?anchor)`)
  }
  // Partial-mock live mode (VITE_NASIKO_MOCK=harnesses): the live user maps onto the seed
  // admin; `?as=` personas are a full-mock-mode feature only (plan §7, N19, E2).
  if (cfg.mode !== 'mock') configureMocks({ persona: null, lockPersona: true })
  // Full mock mode: a sign-out survives a reload, as the server's cleared cookie does (QA ISSUE-003).
  if (cfg.mode === 'mock') {
    configureMocks({
      loggedIn: readMockSignedOut() !== true,
      persistLogin: (value) => {
        try {
          if (value) localStorage.removeItem(MOCK_SIGNED_OUT_KEY)
          else localStorage.setItem(MOCK_SIGNED_OUT_KEY, '1')
        } catch {
          // Storage blocked: the sign-out lasts until the next load.
        }
      },
    })
    // The demo starts as a first-time user (the guide opens on /); a picked persona survives a reload.
    configureMocks({
      onboarding: readMockOnboarding(),
      persistOnboarding: (row) => {
        try {
          localStorage.setItem(MOCK_ONBOARDING_KEY, JSON.stringify(row))
        } catch {
          // Storage blocked: the row lasts until the next load.
        }
      },
    })
    // Tabs share one cookie on the real server: follow sign-ins and sign-outs made in other tabs.
    window.addEventListener('storage', (e) => {
      // key null = another tab cleared storage: re-read rather than assume (the real cookie survives that).
      if (e.key === MOCK_SIGNED_OUT_KEY) configureMocks({ loggedIn: e.newValue !== '1' })
      else if (e.key === null) configureMocks({ loggedIn: readMockSignedOut() !== true })
    })
  }
  const worker = setupWorker(...handlers)
  await worker.start({
    // Unmocked requests pass through silently to the Vite proxy (live mode's real server).
    onUnhandledRequest: 'bypass',
    quiet: true,
  })
}
