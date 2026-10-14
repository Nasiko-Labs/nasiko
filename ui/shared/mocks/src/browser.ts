// Ported from nasiko-ui-lab@1b99bdda common/src/mocks/browser.ts. Local edits: no partial-live mode (`startMocks()`
// takes the anchor env value and the edition's handlers instead of the lab's `EnvConfig`; the app supplies its
// edition's handlers, the lab's `@edition/mocks`, since this package never imports an edition); the session is
// pinned to the seed admin, the principal the server-side mock delegation resolves, so `/api/me` always agrees with
// the server's `me` (a `?as=` persona still picks the harness viewer); msw 3 spells the lab's `onUnhandledRequest` as
// `onUnhandledFrame` and the worker url is passed explicitly; the storage keys are `nasiko:` instead of `ui-lab:`.
/**
 * Starts the MSW service worker in the browser. Loaded only via dynamic import from
 * mock-boot.tsx, so none of this ships in a live-mode production bundle (plan A22).
 */
import { setupWorker } from "msw/browser";
import { readAnchor, pinClock } from "./anchor";
import {
  allHandlers,
  configureMocks,
  seedAdminSession,
  type EditionHandlers,
  type MockOnboarding,
} from "./handlers";
import { generateSeed } from "./seed";

/** Mock-only key: the mock session was ended by a sign-out. */
const MOCK_SIGNED_OUT_KEY = "nasiko:mock-signed-out";

/** The worker script `msw init` copies into `public/` (see README.md). */
export const WORKER_URL = "/mockServiceWorker.js";

/** Storage blocked: null, so every load starts signed in, as before. */
function readMockSignedOut(): boolean | null {
  try {
    return localStorage.getItem(MOCK_SIGNED_OUT_KEY) === "1";
  } catch {
    return null;
  }
}

/** Mock-only key: the mock user's onboarding row, so a finished guide stays finished across reloads. */
const MOCK_ONBOARDING_KEY = "nasiko:mock-onboarding";

/** A first-time user unless a finished guide was stored; storage blocked: first-time on every load. */
function readMockOnboarding(): MockOnboarding {
  try {
    const raw = localStorage.getItem(MOCK_ONBOARDING_KEY);
    if (raw) return JSON.parse(raw) as MockOnboarding;
  } catch {
    // Unreadable: start over.
  }
  return { persona: null, completed: false };
}

export interface StartMocksOptions {
  /** `VITE_NASIKO_SEED_ANCHOR` (the lab's `VITE_NASIKO_SEED_ANCHOR`); `?anchor=` wins. */
  envAnchor?: string | undefined;
  /** The edition's own handlers (EE: as nasiko-server-ee answers), stacked in front of the core's. */
  editionHandlers?: EditionHandlers | undefined;
}

export async function startMocks(opts: StartMocksOptions = {}): Promise<void> {
  // The edition's own mocks answer first (EE: as nasiko-server-ee); this module is reached only by the mock-mode
  // dynamic import, so they never ship in a live bundle.
  const handlers = [...Object.values(opts.editionHandlers ?? {}).flat(), ...allHandlers];
  // Pinned demo time: `?anchor=YYYY-MM-DD` or VITE_NASIKO_SEED_ANCHOR.
  const anchor = readAnchor(location.search, opts.envAnchor);
  if (anchor) {
    pinClock(anchor);
    configureMocks({ seed: generateSeed({ anchor }), now: () => Date.now() });
    console.info(`[nasiko] demo time pinned to ${anchor.toISOString()} (?anchor)`);
  }
  // The mock session is the seed admin: the mock `/api/me` answers with it.
  configureMocks({ session: seedAdminSession });
  // A sign-out survives a reload, as the server's cleared cookie does (QA ISSUE-003).
  configureMocks({
    loggedIn: readMockSignedOut() !== true,
    persistLogin: (value) => {
      try {
        if (value) localStorage.removeItem(MOCK_SIGNED_OUT_KEY);
        else localStorage.setItem(MOCK_SIGNED_OUT_KEY, "1");
      } catch {
        // Storage blocked: the sign-out lasts until the next load.
      }
    },
  });
  // The demo starts as a first-time user (the guide opens on /); a picked persona survives a reload.
  configureMocks({
    onboarding: readMockOnboarding(),
    persistOnboarding: (row) => {
      try {
        localStorage.setItem(MOCK_ONBOARDING_KEY, JSON.stringify(row));
      } catch {
        // Storage blocked: the row lasts until the next load.
      }
    },
  });
  // Tabs share one cookie on the real server: follow sign-ins and sign-outs made in other tabs.
  window.addEventListener("storage", (e) => {
    // key null = another tab cleared storage: re-read rather than assume (the real cookie survives that).
    if (e.key === MOCK_SIGNED_OUT_KEY) configureMocks({ loggedIn: e.newValue !== "1" });
    else if (e.key === null) configureMocks({ loggedIn: readMockSignedOut() !== true });
  });
  const worker = setupWorker(...handlers);
  await worker.start({
    serviceWorker: { url: WORKER_URL },
    // Unmocked requests (Vite's own, fonts, HMR) pass through silently.
    onUnhandledFrame: "bypass",
    quiet: true,
  });
}
