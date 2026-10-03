/**
 * The signed-in layout (plans/feat-app-shell.md §4.1): sidebar + page area. Rendered by the `_app`
 * route, so /login stays bare. Pages keep their own max width; Chat fills the viewport height.
 */
import { Link, Outlet, useRouterState } from '@tanstack/react-router'
import { type ReactNode, useState } from 'react'
import { Button } from '@/components/ui/button'
import { SidebarInset, SidebarProvider, SidebarTrigger } from '@/components/ui/sidebar'
import { cn } from '@/lib/utils'
import { AppSidebar, type NavBadges } from './AppSidebar'
import { copy } from './copy'
import { SidebarPanelContext } from './panelSlot'
import { widthDefaultOpen, readSidebarCookie } from './sidebarState'

/** Chat and Settings own their scrolling (their section column stays put): the page fills the viewport (plan §7.1, EN20). */
const FULL_HEIGHT = /^\/(chat|settings)(\/|$)/

/**
 * `end` and `badges` come from the `_app` route, which may import features (the shell never does): Deploy's build
 * toasts and the Builds item's in-progress count.
 */
export function AppShell({
  end,
  badges,
  hidden,
}: {
  end?: ReactNode
  badges?: NavBadges
  /** Nav paths to leave out (onboarding hides Overview while its guide stands in for it). */
  hidden?: readonly string[]
} = {}) {
  const fullHeight = useRouterState({ select: (s) => FULL_HEIGHT.test(s.location.pathname) })
  // Controlled (v1c E1): the cookie wins; with none, the width default. Chat used to start as the icon rail so its
  // history column fit beside the nav; its history is the sidebar's drill-in panel now, so it needs the open sidebar.
  // Only a user toggle sets `choice`, and the sidebar writes the cookie then, so the default is never stored.
  const [choice, setChoice] = useState(readSidebarCookie)
  const [widthDefault] = useState(widthDefaultOpen)
  const open = choice ?? widthDefault
  // The drill-in slot (sidebarPanel.ts): the sidebar hands out `target`, a page's SidebarPanel portals into it.
  const [target, setTarget] = useState<HTMLElement | null>(null)
  const [panels, setPanels] = useState(0)
  return (
    <SidebarPanelContext value={{ target, setTarget, panels, setPanels }}>
      <SidebarProvider
        open={open}
        onOpenChange={setChoice}
        className={fullHeight ? 'h-dvh' : 'min-h-screen'}
      >
        {/* A button, not an `#main` link: a fragment navigation would add a history entry and make
          the router reload the route (review: red team). */}
        <Button
          variant="outline"
          size="sm"
          onClick={() => document.getElementById('main')?.focus()}
          className="sr-only z-50 focus:not-sr-only focus:fixed focus:top-2 focus:left-2"
        >
          {copy.skipToContent}
        </Button>
        <AppSidebar badges={badges} hidden={hidden} />
        <SidebarInset
          id="main"
          tabIndex={-1}
          className={cn('outline-none', fullHeight ? 'min-h-0' : 'mx-auto max-w-page')}
        >
          {/* Phones: a slim top bar with the menu button opens the nav sheet (§4.2). */}
          <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border px-2 md:hidden">
            <SidebarTrigger aria-label={copy.openMenu} className="size-11" />
            <Link
              to="/"
              activeOptions={{ exact: true }}
              className="flex h-11 items-center gap-2 rounded-md px-2 text-sm font-semibold focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background focus-visible:outline-none"
            >
              <img src="/mark-nasiko.svg" alt="" aria-hidden className="size-4" />
              {copy.brand}
            </Link>
          </div>
          {fullHeight ? (
            <div className="min-h-0 flex-1">
              <Outlet />
            </div>
          ) : (
            <div className="w-full px-4 py-4">
              <Outlet />
            </div>
          )}
        </SidebarInset>
        {end}
      </SidebarProvider>
    </SidebarPanelContext>
  )
}
