/**
 * Sign out (plans/feat-app-shell.md §4.2b, eng D6 / R5).
 *
 * Order matters: in-flight mutations settle first (2 s cap), then local state is cleared, then the
 * logout call is awaited under the session lock (5 s cap), and only then does the app show /login. Showing the login form earlier would let a quick new sign-in be undone
 * by the still-running logout. While this runs, the 401 handler stands down (`isSigningOut`), so a
 * refetch that fails mid-way can't bounce the user to `/login?expired=true`.
 *
 * One sign-out at a time: a second call (double click, a remounted menu) gets the same promise.
 */
import type { QueryClient } from '@tanstack/react-query'
import { logout } from '@/lib/api/auth'
import { isDeadSession401 } from '@/lib/api/detect'
import { clearStoredDrafts } from '@/lib/draftKeys'
import {
  broadcastSession,
  clearSignedOutMark,
  markSignedOutLocally,
  setSigningOut,
  signInGeneration,
  withSessionLock,
} from '@/lib/session'

export const LOGOUT_TIMEOUT_MS = 5_000
/** Sign-in holds the session lock, so it gets a cap too (review, performance + security). */
export const LOGIN_TIMEOUT_MS = 10_000
/** How long sign out waits for in-flight mutations, so their callbacks land before the last clear. */
export const MUTATION_SETTLE_MS = 2_000
/** A chat chunk that can't load in this time is treated as never loaded. */
const IMPORT_TIMEOUT_MS = 2_000

/** `superseded`: a newer sign-in (another tab) happened during this sign-out, so its session was left alone. */
export type SignOutResult = 'signed-out' | 'logout-failed' | 'superseded'

interface SignOutOptions {
  queryClient: QueryClient
  userId: string | undefined
  navigate: (to: { to: '/login'; search: { signout?: 'failed' } }) => Promise<void>
}

let inFlight: Promise<SignOutResult> | null = null

/** Resolves once no sign-out is running in this tab (a sign-in waits for it: review, red team). */
export function whenSignedOut(): Promise<void> {
  // Wait for it whether or not it succeeded: a failed sign-out must not fail the next sign-in.
  return inFlight
    ? inFlight.then(
        () => undefined,
        () => undefined,
      )
    : Promise.resolve()
}

/**
 * Clears this user's local state, ends the server session and goes to /login. A failed or slow
 * logout still ends on /login, with `?signout=failed` so the page can say so and offer Try again.
 * Theme, accent and sidebar preferences are kept: they aren't personal data.
 */
export function signOut(opts: SignOutOptions): Promise<SignOutResult> {
  inFlight ??= run(opts).finally(() => {
    inFlight = null
  })
  return inFlight
}

/** POST /api/auth/logout, capped. Only a success or a dead-session 401 counts as signed out. */
export async function endServerSession(): Promise<SignOutResult> {
  try {
    await logout(LOGOUT_TIMEOUT_MS)
    return 'signed-out'
  } catch (err) {
    // Only a dead-session reason means already signed out (lib/api/detect.ts).
    return isDeadSession401(err) ? 'signed-out' : 'logout-failed'
  }
}

async function run(opts: SignOutOptions): Promise<SignOutResult> {
  setSigningOut(true)
  const generation = signInGeneration()
  try {
    // Mutations already sent finish with the still-valid session. Wait for them before anything
    // is cleared (clear() also empties the mutation cache, which would hide them), so their
    // onSuccess writes land now and are cleared below, not in the next account's cache.
    await settleMutations(opts.queryClient)
    // clear() drops pending mutations from the cache, so keep hold of them to retire them again below.
    const pending = pendingMutations(opts.queryClient)
    await clearLocalState(opts.queryClient, opts.userId)
    // Under the session lock: no tab's sign-in can run between this logout and its outcome.
    const result = await withSessionLock(async (): Promise<SignOutResult> => {
      // Someone signed in (another tab) since this sign-out started: that session isn't ours to end.
      if (signInGeneration() !== generation) return 'superseded'
      const r = await endServerSession()
      // A failed logout leaves the HttpOnly cookie valid: keep this browser on /login (review D1).
      if (r === 'logout-failed') markSignedOutLocally()
      else clearSignedOutMark()
      broadcastSession({ type: 'signed-out', failed: r === 'logout-failed' })
      return r
    }).catch((): SignOutResult => {
      if (signInGeneration() !== generation) return 'superseded'
      // The lock never came (another tab hung): the cookie may still be valid, so fail safe.
      markSignedOutLocally()
      broadcastSession({ type: 'signed-out', failed: true })
      return 'logout-failed'
    })
    await opts.navigate({
      to: '/login',
      search: result === 'logout-failed' ? { signout: 'failed' } : {},
    })
    // Pages stayed mounted during the wait: they may have refetched, and a mounted useMutation
    // re-applies its callbacks on every render. Now that they've unmounted, reset once more.
    retire(pending)
    await clearLocalState(opts.queryClient, opts.userId)
    return result
  } finally {
    setSigningOut(false)
  }
}

/**
 * Drop everything the query client holds for the current session: pending mutations lose their
 * callbacks (they can't be cancelled, but can't write any more), queries are cancelled, then the
 * cache is cleared. Used for this tab's sign-out and sign-in (login.tsx); other tabs' sign-outs and
 * sign-ins reload the page instead (sessionSync.ts).
 */
export async function resetQueryClient(queryClient: QueryClient) {
  retire(pendingMutations(queryClient))
  await queryClient.cancelQueries()
  queryClient.clear()
}

/**
 * Aborts every live chat turn. A missing chat chunk means none were ever started in this tab. Used
 * before another tab's sign-out or sign-in reloads this one (a busy turn's beforeunload prompt would
 * hold the old page), and by the login page: a session that expired mid-stream leaves its turn
 * running, and its reply must not be saved under whoever signs in next (review, security).
 */
export async function stopChatTurns() {
  try {
    const { clearChatRegistry } = await withTimeout(
      import('@/features/chat/registry'),
      IMPORT_TIMEOUT_MS,
    )
    clearChatRegistry()
  } catch {
    // Chunk unavailable: no registry exists here.
  }
}

/**
 * Aborts this tab's uploads and forgets them (plans/feat-deploy.md eng review R3), so the next account never sees an
 * earlier account's upload finish. A missing deploy chunk means no upload was ever started here.
 */
export async function stopDeployWork() {
  try {
    const { clearUploads } = await withTimeout(
      import('@/features/deploy/uploads'),
      IMPORT_TIMEOUT_MS,
    )
    clearUploads()
  } catch {
    // Chunk unavailable: no uploads exist here.
  }
}

/**
 * This tab's personal state: chat registry, drafts, uploads, query cache. Never throws: a missing chat
 * chunk means no registry or in-memory drafts were ever created in this tab.
 */
export async function clearLocalState(queryClient: QueryClient, userId: string | undefined) {
  try {
    const [{ clearChatRegistry }, { clearDrafts }] = await withTimeout(
      Promise.all([import('@/features/chat/registry'), import('@/features/chat/drafts')]),
      IMPORT_TIMEOUT_MS,
    )
    clearChatRegistry()
    // No user (the "Account unavailable" row): clear every draft.
    clearDrafts(userId)
  } catch {
    // Chunk unavailable (offline, stale deploy): its in-memory state never existed here, but
    // drafts stored by an earlier page load still sit in localStorage.
    clearStoredDrafts()
  }
  // After chat and drafts (eng review R6 keeps that order): uploads and followed builds.
  await stopDeployWork()
  await resetQueryClient(queryClient)
}

function settleMutations(client: QueryClient): Promise<void> {
  if (client.isMutating() === 0) return Promise.resolve()
  return new Promise((resolve) => {
    const done = () => {
      clearTimeout(timer)
      unsubscribe()
      resolve()
    }
    const timer = setTimeout(done, MUTATION_SETTLE_MS)
    const unsubscribe = client.getMutationCache().subscribe(() => {
      if (client.isMutating() === 0) done()
    })
  })
}

type AnyMutation = ReturnType<ReturnType<QueryClient['getMutationCache']>['getAll']>[number]

/** Pending mutations. Capture them before clear(), which drops them from the cache (but not from flight). */
function pendingMutations(client: QueryClient): AnyMutation[] {
  return client
    .getMutationCache()
    .getAll()
    .filter((m) => m.state.status === 'pending')
}

/**
 * A mutation still running can't be cancelled, but its callbacks can be dropped: TanStack reads
 * `mutation.options` when it settles. A mounted useMutation re-applies them on render, so callers
 * retire again once the pages are gone; per-call callbacks need a listening observer.
 */
function retire(mutations: readonly AnyMutation[]) {
  for (const m of mutations) {
    if (m.state.status !== 'pending') continue
    m.setOptions({ ...m.options, onSuccess: undefined, onError: undefined, onSettled: undefined })
  }
}

function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => reject(new Error('timeout')), ms)
    p.then(
      (v) => {
        clearTimeout(t)
        resolve(v)
      },
      (e: unknown) => {
        clearTimeout(t)
        reject(e instanceof Error ? e : new Error(String(e)))
      },
    )
  })
}
