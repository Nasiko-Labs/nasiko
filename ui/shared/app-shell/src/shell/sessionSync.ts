// Ported from nasiko-ui-lab@1b99bdda common/src/app/shell/sessionSync.ts. Local edits: the TanStack router (its
// location and `invalidate`/`buildLocation`) becomes a small `location()` reader plus `recheck()` the caller passes
// (the app providers read `window.location` and refetch `me`); the `me` cache entry is the scoped product key
// (`meQueryOptions(scope).queryKey`), so the caller passes `meKey`; the login URL is built with URLSearchParams;
// `stopChatTurns` (signOut.ts) clears the chat registry; Prettier formatting.
/**
 * Keeps this tab in step with sign-outs and sign-ins in the browser's other tabs (review D2):
 * broadcast + reload. A full document load drops every in-memory store, the query cache and
 * pending mutation callbacks at once, so nothing is reset piecemeal here. Installed once at boot
 * (main.tsx).
 */
import type { QueryClient, QueryKey } from "@tanstack/react-query";
import { safeRedirect, type Me } from "@nasiko/data-access/product";
import { isSigningOut, onSessionMessage, signInGeneration } from "@nasiko/app-core/session";
import { stopChatTurns } from "./signOut";

type Page = Pick<Location, "assign" | "replace" | "reload">;

/** Where this tab is: the path, its query and the href a redirect carries (path + query). */
export interface TabLocation {
  pathname: string;
  search: URLSearchParams;
  href: string;
}

/** The browser's own location, as the app providers pass it. */
export const windowLocation = (): TabLocation => ({
  pathname: window.location.pathname,
  search: new URLSearchParams(window.location.search),
  href: window.location.pathname + window.location.search + window.location.hash,
});

export function watchOtherTabs(opts: {
  queryClient: QueryClient;
  /** The `me` query's key (`meQueryOptions(scope).queryKey`). */
  meKey: QueryKey;
  /** Where this tab is, read when a message arrives. */
  location: () => TabLocation;
  /** The guard's re-check (the lab's `router.invalidate()`): refetch `me` without dropping anything. */
  recheck: () => void;
  /** The document to navigate; tests pass a fake (jsdom can't navigate). */
  page?: Page;
}): () => void {
  const page = opts.page ?? window.location;
  // Stop live chat turns first: a busy turn's beforeunload prompt would otherwise let the user stay
  // on the old session's page.
  const leave = (go: () => void) => void stopChatTurns().then(go);
  return onSessionMessage((message) => {
    const loc = opts.location();
    const search = loc.search;
    const onLogin = loc.pathname === "/login";
    if (message.type === "signed-out") {
      // Whoever was signed in here is gone: reload into /login, back to this page after a sign-in.
      const signout = message.failed ? "failed" : undefined;
      const params = onLogin
        ? new URLSearchParams(search)
        : new URLSearchParams({ redirect: loc.href });
      params.delete("signout");
      if (signout) params.set("signout", signout);
      const to = `/login?${params.toString()}`;
      const generation = signInGeneration();
      leave(() => {
        // This tab signed in while the chat chunk loaded: that new session stays.
        if (signInGeneration() !== generation) return;
        if (onLogin) page.replace(to);
        else page.assign(to);
      });
    } else if (!isSigningOut()) {
      // (While this tab signs out, its own flow decides; a newer sign-in makes it stand down.)
      // ponytail: a /login tab mid-submit is navigated away too; its own sign-in is moot once another tab signed in.
      if (onLogin) {
        leave(() => {
          page.assign(safeRedirect(search.get("redirect") ?? undefined));
        });
        return;
      }
      // The same account signed in again (say, after its session expired in that tab): everything here
      // still belongs to it, so keep pages, typed input and chat turns, and only let the guard re-check.
      const shown = opts.queryClient.getQueryData<Me>(opts.meKey)?.sub;
      if (message.sub && shown === message.sub) {
        opts.recheck();
        return;
      }
      // Another account (or one we can't tell): nothing on this page is theirs.
      leave(() => {
        page.reload();
      });
    }
  });
}
