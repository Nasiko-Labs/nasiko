/**
 * The signed-in layout (plans/feat-app-shell.md §4.1): sidebar + page area. Rendered by the `_app`
 * route, so /login stays bare. Pages keep their own max width; Chat fills the viewport height.
 */
import { Outlet, useRouterState } from '@tanstack/react-router'
import { type ComponentType, type ReactNode, use, useState } from 'react'
import { applyNav } from '@nasiko/app-core/edition/edition'
import { EditionContext } from '@nasiko/app-core/edition/edition-context'
import { Button } from '@nasiko/ui/primitives/button'
import { SidebarInset, SidebarProvider } from '@nasiko/ui/primitives/sidebar'
import { cn } from '@nasiko/ui/lib/utils'
import { AppSidebar, type NavBadges } from './AppSidebar'
import { copy } from '@nasiko/app-core/shell/copy'
import { activeItem, NAV_GROUPS, NAV_ITEMS } from '@nasiko/app-core/shell/nav'
import { StaleBuildNotice } from './StaleBuildNotice'
import { ShellTopBar } from '@nasiko/app-core/shell/TopBar'
import { AssessmentBanner } from './AssessmentCta'
import { useAssessmentBanner } from './assessmentBanner'
import { SidebarPanelContext } from '@nasiko/app-core/shell/panelSlot'
import { readSidebarCookie } from '@nasiko/app-core/shell/sidebarState'

/** Chat, Settings and Account own their scrolling (their section column stays put): the page fills the viewport (plan §7.1, EN20). */
const FULL_HEIGHT = /^\/(chat|settings|account)(\/|$)/
/** Pages without the top bar from `md` up (phones keep it for the menu button). */
const BARE = /^\/(chat\/?)?$/

/**
 * `end` and `badges` come from the `_app` route, which may import features (the shell never does): Deploy's build
 * toasts and the Builds item's in-progress count.
 */
export function AppShell({
  end,
  split: Split,
  badges,
  hidden,
}: {
  end?: ReactNode
  split?: ComponentType<{ path: string; children: ReactNode }> | null
  badges?: NavBadges
  /** Nav paths to leave out (onboarding hides Home while its guide stands in for it). */
  hidden?: readonly string[]
} = {}) {
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  const fullHeight = FULL_HEIGHT.test(pathname)
  // The top bar's start: the page's nav group and item, layer patches applied (as the sidebar does).
  const { edition, slots } = use(EditionContext)
  const active = activeItem(pathname, applyNav(NAV_ITEMS, edition.layers))
  const Notice = slots.shellNotice
  const item = active
    ? {
        to: active.to,
        label: active.label,
        group: NAV_GROUPS.find((g) => g.id === active.group)?.label ?? null,
      }
    : null
  // Overview and New chat have no bar on desktop (user, 2026-10-08): nothing to place, nowhere to step.
  const bare = BARE.test(pathname)
  // Controlled (v1c E1): the cookie wins; with none, the icon rail (user request 2026-11-05: collapsed on sign-in).
  // Only a user toggle sets `choice`, and the sidebar writes the cookie then, so the default is never stored.
  const [choice, setChoice] = useState(readSidebarCookie)
  const open = choice ?? false
  const [bannerShown, dismissBanner] = useAssessmentBanner()
  // Not on the assessment page itself.
  const banner = bannerShown && pathname !== '/assessment'
  // The drill-in slot (sidebarPanel.ts): the sidebar hands out `target`, a page's SidebarPanel portals into it.
  const [target, setTarget] = useState<HTMLElement | null>(null)
  const [panels, setPanels] = useState(0)
  const inset = (
    // min-w-0: a flex item never shrinks below its content, so one wide row (the trace switcher's toggles) pushed
    // the whole page, top bar included, past the window by the sidebar's width; that row scrolls on its own (NAS-1609).
    <SidebarInset
      id="main"
      tabIndex={-1}
      className={cn('min-w-0 outline-none', fullHeight && 'min-h-0')}
    >
      <StaleBuildNotice />
      {Notice ? <Notice /> : null}
      {/* One bar above every page (TopBar.tsx); on phones it also holds the menu button (§4.2). */}
      <ShellTopBar
        item={item}
        pathname={pathname}
        bare={bare}
        banner={banner ? <AssessmentBanner onDismiss={dismissBanner} /> : null}
      >
        {fullHeight ? (
          <div className="min-h-0 flex-1">
            <Outlet />
          </div>
        ) : (
          // `--float-clearance`: room a layer's floating control asks the page to leave at its foot.
          <div className="mx-auto w-full max-w-page px-4 pt-4 pb-[calc(1rem+var(--float-clearance,0px))]">
            <Outlet />
          </div>
        )}
      </ShellTopBar>
    </SidebarInset>
  )
  return (
    <SidebarPanelContext value={{ target, setTarget, panels, setPanels }}>
      <SidebarProvider
        open={open}
        onOpenChange={setChoice}
        // A bare page has no bar to clear: its sticky rows and anchor jumps go to the top, or under the banner.
        className={cn(
          fullHeight ? 'h-dvh' : 'min-h-screen',
          banner && '[--banner-h:2.75rem] [--topbar-h:calc(3.25rem_+_var(--banner-h))]',
          bare && (banner ? 'md:[--topbar-h:var(--banner-h)]' : 'md:[--topbar-h:0px]'),
        )}
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
        {/* Its assessment row takes over once the banner is dismissed. */}
        <AppSidebar badges={badges} hidden={hidden} assessment={!bannerShown} />
        {Split ? <Split path={pathname}>{inset}</Split> : inset}
        {end}
      </SidebarProvider>
    </SidebarPanelContext>
  )
}
