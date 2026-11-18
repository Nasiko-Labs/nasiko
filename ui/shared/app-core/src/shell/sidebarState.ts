/**
 * The sidebar's open/closed state is shadcn's `sidebar_state` cookie, one value for the whole app
 * (eng D2). With no cookie it starts as the icon rail at every width (user request 2026-11-05), and sign-out
 * drops the cookie, so every sign-in starts collapsed. Phones (< 768 px) use the sheet whatever this says.
 */
/** Must equal `SIDEBAR_COOKIE_NAME` in src/components/ui/sidebar.tsx (checked by sidebarState.test.ts). */
export const SIDEBAR_COOKIE = 'sidebar_state'

export function readSidebarCookie(
  cookie = typeof document === 'undefined' ? '' : document.cookie,
): boolean | undefined {
  const m = new RegExp(`(?:^|;\\s*)${SIDEBAR_COOKIE}=(true|false)(?:;|$)`).exec(cookie)
  return m ? m[1] === 'true' : undefined
}

/** Forget the stored choice (sign-out), so the next sign-in starts as the rail. */
export function clearSidebarCookie() {
  if (typeof document !== 'undefined') document.cookie = `${SIDEBAR_COOKIE}=; path=/; max-age=0`
}
