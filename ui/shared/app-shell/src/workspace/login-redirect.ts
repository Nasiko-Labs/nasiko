import { safeRedirect } from "@nasiko/data-access/product";

const ON_LOGIN = /^\/login(?=$|[/?#])/;

/**
 * Where the workspace sends a tab that has no session: /login, back to `href` after a sign-in. `null` when the tab
 * is already on /login.
 *
 * Why `null` matters: navigating to /login is a React transition, and while the login page's chunk loads React keeps
 * the workspace painted, reading the new location. A workspace that redirected from there built
 * `/login?redirect=/login…`, whose redirect then nested again on every render — after a successful sign-out on the
 * real control plane, the tab looped until it crashed. The redirect target also goes through `safeRedirect`, so a
 * /login (or foreign) target can never be nested even if a caller forgets the check.
 */
export function workspaceLoginUrl(href: string, extra: Record<string, string> = {}): string | null {
  if (ON_LOGIN.test(href)) return null;
  return `/login?${new URLSearchParams({ redirect: safeRedirect(href), ...extra }).toString()}`;
}
