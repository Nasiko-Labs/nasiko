/**
 * Keeps this tab in step with sign-outs and sign-ins in the browser's other tabs (review D2):
 * broadcast + reload. A full document load drops every in-memory store, the query cache and
 * pending mutation callbacks at once, so nothing is reset piecemeal here. Installed once at boot
 * (main.tsx).
 */
import type { QueryClient } from '@tanstack/react-query'
import type { AnyRouter } from '@tanstack/react-router'
import { meQuery, safeRedirect, type Me } from '@/lib/api/auth'
import { isSigningOut, onSessionMessage, signInGeneration } from '@/lib/session'
import { stopChatTurns } from './signOut'

type Page = Pick<Location, 'assign' | 'replace' | 'reload'>

export function watchOtherTabs(opts: {
  queryClient: QueryClient
  router: AnyRouter
  /** The document to navigate; tests pass a fake (jsdom can't navigate). */
  page?: Page
}): () => void {
  const page = opts.page ?? window.location
  // Stop live chat turns first: a busy turn's beforeunload prompt would otherwise let the user stay
  // on the old session's page.
  const leave = (go: () => void) => void stopChatTurns().then(go)
  return onSessionMessage((message) => {
    const loc = opts.router.state.location
    const search = (loc.search ?? {}) as Record<string, unknown>
    const onLogin = loc.pathname === '/login'
    if (message.type === 'signed-out') {
      // Whoever was signed in here is gone: reload into /login, back to this page after a sign-in.
      const signout = message.failed ? 'failed' : undefined
      const to = opts.router.buildLocation({
        to: '/login',
        search: onLogin ? { ...search, signout } : { redirect: loc.href, signout },
      }).href
      const generation = signInGeneration()
      leave(() => {
        // This tab signed in while the chat chunk loaded: that new session stays.
        if (signInGeneration() !== generation) return
        if (onLogin) page.replace(to)
        else page.assign(to)
      })
    } else if (!isSigningOut()) {
      // (While this tab signs out, its own flow decides; a newer sign-in makes it stand down.)
      // ponytail: a /login tab mid-submit is navigated away too; its own sign-in is moot once another tab signed in.
      if (onLogin) return leave(() => page.assign(safeRedirect(search.redirect)))
      // The same account signed in again (say, after its session expired in that tab): everything here
      // still belongs to it, so keep pages, typed input and chat turns, and only let the guard re-check.
      const shown = opts.queryClient.getQueryData<Me>(meQuery.queryKey)?.sub
      if (message.sub && shown === message.sub) return void opts.router.invalidate()
      // Another account (or one we can't tell): nothing on this page is theirs.
      leave(() => page.reload())
    }
  })
}
