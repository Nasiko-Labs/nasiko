// Destination file (no lab counterpart: the lab's single bundle had loaded before its router's guard asked for the
// session). In the SPA the signed-in bootstrap's `GET /api/me` lived in WorkspaceLayout, a lazy chunk, so a cold load
// requested it only after the layout's and the route's code had downloaded and run (measured on the release build:
// ~150 script requests, then /api/me; docs/migration/spa-stabilization-2026-10-13.md). This starts the same request as
// soon as the router exists, for a URL the workspace layout guards, so the session check overlaps the code download.
//
// Only the caller's identity is requested (no workspace data). WorkspaceLayout takes the promise once, for its first
// load, and applies its usual rules to the answer (a superseded or unmounted bootstrap discards it); every later
// bootstrap (refresh, revocation, identity change) fetches anew. A `nasiko:session-refresh` before the layout has taken
// the promise (the layout's code is still loading, so nobody else hears it) aborts and drops it, so the layout fetches
// an answer that is newer than the refresh (tests/e2e/identity.e2e.ts, the superseded-bootstrap case). Not in mock
// mode: the mock worker is not running yet.
import type { AnyRouter } from "@tanstack/react-router";
import { env } from "@nasiko/app-core/env";
import { isSignedOutLocally } from "@nasiko/app-core/session";
import { fetchMe, type Me } from "@nasiko/data-access/product";

/** The route id of the guarded layout (app-shell/router.tsx `createRoute({ id: "workspace" })`). */
const WORKSPACE_ROUTE_ID = "/workspace";

const REFRESH_EVENT = "nasiko:session-refresh";

let early: Promise<Me> | null = null;
let discard: (() => void) | null = null;

/** Start `GET /api/me` now when the current URL is a guarded workspace route and a session may exist. */
export function startEarlySession(router: AnyRouter): void {
  if (env.mode !== "live" || isSignedOutLocally()) return;
  const [matched] = router.getMatchedRoutes(router.state.location.pathname);
  const guarded = matched.some((route) => route.id === WORKSPACE_ROUTE_ID);
  if (!guarded) return;
  const controller = new AbortController();
  const request = fetchMe({ signal: controller.signal });
  // WorkspaceLayout handles the outcome; a request nobody takes must not surface as an unhandled rejection.
  request.catch(() => undefined);
  early = request;
  discard = () => {
    controller.abort();
    release();
  };
  window.addEventListener(REFRESH_EVENT, discard);
}

/** Forget the early request (and its refresh listener). */
function release(): void {
  if (discard) window.removeEventListener(REFRESH_EVENT, discard);
  discard = null;
  early = null;
}

/** The early request, once: the first bootstrap takes it, every later one fetches anew. */
export function takeEarlySession(): Promise<Me> | null {
  const request = early;
  release();
  return request;
}
