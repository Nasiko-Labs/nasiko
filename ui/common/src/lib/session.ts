/**
 * App-wide session state shared by the query client, the `_app` guard, the login page and the
 * app shell. Kept in `lib` so none of them depends on a feature module.
 */

let signingOut = false

/** True while a sign-out is in progress: the 401 handler stands down (plans/feat-app-shell.md eng D6). */
export function isSigningOut(): boolean {
  return signingOut
}

export function setSigningOut(value: boolean) {
  signingOut = value
}

/**
 * Local sign-out barrier (review D1). The login cookie is HttpOnly: after a failed logout it may
 * still be valid, so this browser remembers "signed out here" and the `_app` guard sends every
 * page to /login until a sign-in or a successful Try again clears it. Storage blocked: no barrier.
 */
export const SIGNED_OUT_KEY = 'openruntime.signedOut'

export function markSignedOutLocally() {
  try {
    window.localStorage.setItem(SIGNED_OUT_KEY, '1')
  } catch {
    // Storage blocked: the notice on /login still says the browser may be signed in.
  }
}

export function clearSignedOutMark() {
  try {
    window.localStorage.removeItem(SIGNED_OUT_KEY)
  } catch {
    // Nothing stored.
  }
}

export function isSignedOutLocally(): boolean {
  return signedOutMark() === true
}

/** The barrier's state: set, not set, or unknown (storage blocked, so no barrier can exist). */
export function signedOutMark(): boolean | null {
  try {
    return window.localStorage.getItem(SIGNED_OUT_KEY) === '1'
  } catch {
    return null
  }
}

/**
 * Logout and sign-in share one cookie across every tab, so they must never interleave: a late
 * logout would clear a new sign-in's cookie. This Web Lock serialises them browser-wide (review,
 * red team). Without Web Locks (old browsers, tests) the call just runs.
 */
export const SESSION_LOCK = 'openruntime.session'
/**
 * Longest wait for the lock. Work under it is itself capped (logout and login calls time out), so
 * this only trips if another tab hung; the caller then treats the operation as failed.
 */
export const SESSION_LOCK_WAIT_MS = 15_000

export function withSessionLock<T>(
  fn: () => Promise<T>,
  locks: Pick<LockManager, 'request'> | null = defaultLocks(),
): Promise<T> {
  if (!locks) return fn()
  return locks.request(SESSION_LOCK, { signal: AbortSignal.timeout(SESSION_LOCK_WAIT_MS) }, () =>
    fn(),
  ) as Promise<T>
}

function defaultLocks(): Pick<LockManager, 'request'> | null {
  // Web Locks need a secure context (https or localhost); elsewhere the calls just run, and the
  // sign-in generation plus stamped messages are the only cross-tab protection.
  // gstack-shortcut(dec-2dc6353e-fd7b-4645-8fc6-82683376f125): no cross-tab lock over plain HTTP, upgrade when the SPA ships inside nasiko-server over http:// or a user is signed out right after signing in
  return typeof navigator !== 'undefined' && 'locks' in navigator ? navigator.locks : null
}

/**
 * Sign-in generation (review, security): bumped by every sign-in under the session lock. A sign-out
 * snapshots it before its local cleanup and skips the logout if a newer sign-in happened meanwhile,
 * so it never ends a session some other tab just started. Chat's pending-requests poll also stamps
 * its results with it (`features/chat/api.ts`).
 */
export const SIGNIN_GENERATION_KEY = 'openruntime.signinGeneration'

export function signInGeneration(): string {
  try {
    return window.localStorage.getItem(SIGNIN_GENERATION_KEY) ?? '0'
  } catch {
    return '0'
  }
}

export function bumpSignInGeneration() {
  try {
    window.localStorage.setItem(
      SIGNIN_GENERATION_KEY,
      `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    )
  } catch {
    // Storage blocked: no cross-tab protection beyond the lock.
  }
}

/**
 * Tells this browser's other tabs about sign-outs and sign-ins (review D2), so no tab keeps showing
 * a signed-out user's data or mixes two accounts. BroadcastChannel never echoes to its own sender.
 *
 * `generation` on a sign-out is the sign-in generation it ended (stamped by broadcastSession). A
 * receiver whose stored generation differs has seen a newer sign-in since, so it drops the message:
 * the lock orders the HTTP calls, not the messages (review, red team).
 *
 * `sub` on a sign-in is the new user's id (the login response's `user_id`, which is the JWT `sub`), so other
 * tabs can tell a same-account sign-in (keep everything) from an account switch (reload the page).
 */
export type SessionMessage =
  { type: 'signed-out'; failed: boolean; generation?: string } | { type: 'signed-in'; sub?: string }

export const SESSION_CHANNEL = 'openruntime.session'

let channel: BroadcastChannel | null | undefined

function getChannel(): BroadcastChannel | null {
  if (channel === undefined)
    channel = typeof BroadcastChannel === 'function' ? new BroadcastChannel(SESSION_CHANNEL) : null
  return channel
}

export function broadcastSession(message: SessionMessage) {
  const stamped =
    message.type === 'signed-out' ? { ...message, generation: signInGeneration() } : message
  try {
    getChannel()?.postMessage(stamped)
  } catch {
    // Channel closed or unavailable: other tabs find out on their next request.
  }
}

/** Subscribe to other tabs' session messages; returns the unsubscribe. */
export function onSessionMessage(listener: (message: SessionMessage) => void): () => void {
  const ch = getChannel()
  if (!ch) return () => {}
  // Same-origin only, but a stale deploy or a stray script could post anything: accept known shapes.
  const handler = (e: MessageEvent<unknown>) => {
    const m = e.data as Partial<SessionMessage> | null
    if (m?.type === 'signed-in')
      listener(
        typeof m.sub === 'string' && m.sub
          ? { type: 'signed-in', sub: m.sub }
          : { type: 'signed-in' },
      )
    else if (m?.type === 'signed-out' && typeof m.failed === 'boolean') {
      // A sign-out older than this browser's latest sign-in is stale: that session was left alone.
      if (typeof m.generation === 'string' && m.generation !== signInGeneration()) return
      listener({ type: 'signed-out', failed: m.failed })
    }
  }
  ch.addEventListener('message', handler)
  return () => ch.removeEventListener('message', handler)
}
