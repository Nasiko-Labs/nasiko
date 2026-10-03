/**
 * The sidebar's open/closed state is shadcn's `sidebar_state` cookie, one value for the whole app
 * (eng D2). With no cookie yet, the first visit opens it at 1280 px and up and starts as the icon
 * rail below that (design review 9A), except on Chat, which starts as the rail (v1c D3). Phones (< 768 px)
 * use the sheet whatever this says.
 */
/** Must equal `SIDEBAR_COOKIE_NAME` in src/components/ui/sidebar.tsx (checked by sidebarState.test.ts). */
export const SIDEBAR_COOKIE = 'sidebar_state'
export const EXPANDED_MIN_WIDTH = 1280

export function readSidebarCookie(
  cookie = typeof document === 'undefined' ? '' : document.cookie,
): boolean | undefined {
  const m = new RegExp(`(?:^|;\\s*)${SIDEBAR_COOKIE}=(true|false)(?:;|$)`).exec(cookie)
  return m ? m[1] === 'true' : undefined
}

/** Open or rail with no cookie, by window width (design review 9A); Chat overrides it (v1c D3, AppShell). */
export function widthDefaultOpen(): boolean {
  return typeof window !== 'undefined' && window.innerWidth >= EXPANDED_MIN_WIDTH
}
