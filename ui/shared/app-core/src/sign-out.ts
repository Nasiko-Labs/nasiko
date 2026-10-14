// No lab counterpart: the seam a feature signs out through. In the lab a page imported `signOut` from
// `@/app/shell/signOut` (common/src/features/settings/PasswordPage.tsx); here a feature cannot import the shell
// (architecture rule), so the shell provides its `signOut` through this context. Local edits: n/a (new file).
"use client";

import { createContext, use } from "react";

/** Where the shell's sign-out lands: `/login` with the search it chose (`signout=failed` when the logout failed). */
export interface SignOutTarget {
  to: "/login";
  search: { signout?: "failed" };
}

/**
 * The shell's sign-out (plans/feat-app-shell.md §4.2b): settles mutations, clears this user's local state, ends the
 * server session under the session lock, then calls `navigate` with where it would go. The caller navigates (the
 * Password page adds `?password=changed` unless the logout failed), as the lab's callers passed `navigate`. The
 * query client is the shell's own.
 */
export type SignOut = (opts: {
  userId: string | undefined;
  navigate: (to: SignOutTarget) => Promise<void>;
}) => Promise<unknown>;

export const SignOutContext = createContext<SignOut | null>(null);

/** The shell's sign-out. Throws outside the signed-in providers (nothing could sign out there). */
export function useSignOut(): SignOut {
  const signOut = use(SignOutContext);
  if (!signOut) throw new Error("useSignOut: no SignOutContext (render inside the app providers)");
  return signOut;
}
