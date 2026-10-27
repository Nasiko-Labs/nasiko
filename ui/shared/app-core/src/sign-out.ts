/**
 * The seam a feature signs out through. A feature never imports the shell (eslint.config.js BELOW_SHELL), and the
 * shell's `signOut` (shared/app-shell/src/shell/signOut.ts) clears feature state on its way out, so the shell hands it
 * down through this context instead: the root route (shared/app-shell/src/routes/__root.tsx) provides it. After
 * codex/nasiko-react-foundation ui/shared/app-core/src/sign-out.ts.
 */
import type { QueryClient } from '@tanstack/react-query'
import { createContext, use } from 'react'

/** Where the shell's sign-out lands: `/login`, with `signout=failed` when the logout failed. */
export interface SignOutTarget {
  to: '/login'
  search: { signout?: 'failed' }
}

/** The shell's sign-out: settles mutations, clears this user's local state, ends the server session, then navigates. */
export type SignOut = (opts: {
  queryClient: QueryClient
  userId: string | undefined
  navigate: (to: SignOutTarget) => Promise<void>
}) => Promise<unknown>

export const SignOutContext = createContext<SignOut | null>(null)

/** The shell's sign-out. Throws outside the app's providers (nothing could sign out there). */
export function useSignOut(): SignOut {
  const signOut = use(SignOutContext)
  if (!signOut) throw new Error('useSignOut: no SignOutContext (render inside the app providers)')
  return signOut
}
