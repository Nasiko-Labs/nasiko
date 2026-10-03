/**
 * The app's left sidebar (plans/feat-app-shell.md §3–§4): the header (OpenRuntime home link, collapse), the nav groups
 * from `nav.ts` or a page's drill-in panel (`SidebarPanel`: Chat's history, Settings' sections), and the footer
 * (status in local builds, account; Settings opens from the account menu). Built on shadcn's sidebar primitive.
 */
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useRouter, useRouterState } from '@tanstack/react-router'
import {
  ArrowLeft,
  CircleAlert,
  CircleUser,
  LogOut,
  Palette,
  PanelLeftClose,
  PanelLeftOpen,
  RotateCw,
  Settings,
} from 'lucide-react'
import {
  use,
  useCallback,
  useId,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type RefObject,
} from 'react'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from '@/components/ui/sidebar'
import { applyNav, type AnyNavItem } from '@/app/edition'
import { EditionContext } from '@/app/edition-context'
import { meQuery } from '@/lib/api/auth'
import { ApiError } from '@/lib/api/client'
import { SIDEBAR_HEALTH_INTERVAL_MS, useHealth } from '@/lib/api/health'
import { env } from '@/lib/env'
import { cn } from '@/lib/utils'
import { pickShared, withoutWindow } from './context'
import { copy } from './copy'
import { NasikoMark } from './NasikoMark'
import { activeItem, backIndex, moduleOf, NAV_GROUPS, NAV_ITEMS, pathOf } from './nav'
import { ACTIVE_ROW, GROUP, LABEL, ROW } from './rowStyles'
import { signOut } from './signOut'
import {
  ACCENTS,
  setAccent,
  setTheme,
  THEMES,
  useThemePrefs,
  type Accent,
  type Theme,
} from './theme'
import { SidebarPanelContext } from './panelSlot'
import { WaitlistCard } from './WaitlistCta'

/** How long a pending health check stays quiet before "Checking…" shows (design review 2A). */
const CHECKING_DELAY_MS = 1_000

/**
 * The status row is for whoever runs the lab: local dev servers, and any build that serves mock data (so a demo
 * build still says MOCK DATA). A production build against a real server leaves it out, and polls no /health.
 */
const SHOW_STATUS = import.meta.env.DEV || env.mode === 'mock'

/** Which view takes focus after a swap between the app nav and a drill-in panel (the clicked row unmounts). */
type FocusNext = RefObject<'menu' | 'panel' | null>

/** A count on a nav item, with the words a screen reader hears after its label (the Builds item: builds in progress). */
export type NavBadges = Partial<Record<string, { count: number; label: string }>>

export function AppSidebar({
  badges,
  hidden,
}: { badges?: NavBadges; hidden?: readonly string[] } = {}) {
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  const href = useRouterState({ select: (s) => s.location.href })
  const navigate = useNavigate()
  const layers = use(EditionContext).edition.layers
  const { setOpenMobile, isMobile, state } = useSidebar()
  const { panels, setTarget } = use(SidebarPanelContext)
  // The phone sheet closes on navigation (§4.2).
  useEffect(() => setOpenMobile(false), [pathname, setOpenMobile])
  // "Main menu" shows the app nav over a page's panel until the next navigation (or a click on the current item).
  const [menu, setMenu] = useState(false)
  const [menuPath, setMenuPath] = useState(pathname)
  if (menuPath !== pathname) {
    setMenuPath(pathname)
    setMenu(false)
  }
  const focusNextRef = useRef<'menu' | 'panel' | null>(null)
  // A click on the current page's item (nav or footer) brings its panel back: the path doesn't change.
  const pick = () => {
    if (panels > 0) focusNextRef.current = 'panel'
    setMenu(false)
  }
  // The collapsed rail keeps the app nav's icons; the page then shows its panel's fallback itself.
  const holds = panels > 0 && (isMobile || state === 'expanded')
  const drilled = holds && !menu
  // The pages visited in this tab (newest last), so the panel's Back returns to where the user came from.
  // Appended when the page changes, so Back's trimming (before its navigation lands) never re-adds the page it leaves.
  const [trail, setTrail] = useState<readonly string[]>([href])
  const [trailHref, setTrailHref] = useState(href)
  if (trailHref !== href) {
    setTrailHref(href)
    setTrail([...trail.slice(1 - TRAIL_LENGTH), href])
  }
  const backAt = backIndex(trail, pathname)
  const back = trail[backAt]
  const backLabel = back ? labelFor(pathOf(back), applyNav(NAV_ITEMS, layers)) : null

  return (
    <Sidebar collapsible="icon">
      <SidebarHeader className={cn(GROUP, 'py-3')}>
        <SidebarMenu className="flex-row items-center gap-1">
          {/* The rail has room for one control: the mark, which turns into Expand on hover or focus. */}
          {state === 'collapsed' && !isMobile ? (
            <ExpandMark />
          ) : (
            <>
              <SidebarMenuItem className="min-w-0 flex-1">
                <SidebarMenuButton asChild tooltip={copy.brandTooltip} className={cn(ROW, 'h-10')}>
                  {/* A plain link: TanStack's Link would mark it aria-current on /, doubling the nav's Overview item
                      (§3 rule 4: one current item). */}
                  <a
                    href="/"
                    onClick={(e) => {
                      if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return
                      e.preventDefault()
                      void navigate({ to: '/' })
                    }}
                  >
                    <NasikoMark className="size-4 shrink-0 text-logo" />
                    <span className={cn(LABEL, 'flex min-w-0 flex-col leading-tight')}>
                      <span className="truncate font-semibold">{copy.brand}</span>
                    </span>
                  </a>
                </SidebarMenuButton>
              </SidebarMenuItem>
              <CollapseButton />
            </>
          )}
        </SidebarMenu>
      </SidebarHeader>
      {drilled ? (
        <BackRow
          focusNextRef={focusNextRef}
          name={backLabel ? copy.backTo(backLabel) : back ? copy.back : copy.backToMenu}
          onBack={() => {
            focusNextRef.current = 'menu'
            // Opened directly (no page before it): show the app nav instead.
            if (back === undefined) return setMenu(true)
            // Like history: the target and what came after it leave the trail (the target is appended again).
            setTrail(trail.slice(0, backAt))
            void navigate({ href: back })
          }}
        />
      ) : (
        <NavGroups
          pathname={pathname}
          badges={badges}
          hidden={hidden}
          focusNextRef={focusNextRef}
          onPick={pick}
        />
      )}
      {/* The panel's slot stays mounted (only hidden) under Main menu: the page still knows the sidebar holds its
          panel, so it doesn't show its fallback beside the page, and the panel keeps its state. */}
      {holds ? (
        <div
          ref={setTarget}
          // The attribute, not a class: Tailwind's preflight hides [hidden], and it leaves the accessibility tree too.
          hidden={!drilled}
          className="flex min-h-0 flex-1 flex-col"
        />
      ) : null}
      {/* Not while a page's panel holds the sidebar: Chat's history and Settings' sections keep the room, and Chat's dot
          background stays its page's one effect (CLAUDE.md, Aceternity effects). */}
      {holds ? null : <WaitlistCard />}
      <SidebarFooter className={cn(GROUP, 'gap-0 border-t border-sidebar-border py-2')}>
        <SidebarMenu>
          {SHOW_STATUS ? <StatusRow /> : null}
          <AccountMenu onPick={pick} />
        </SidebarMenu>
      </SidebarFooter>
    </Sidebar>
  )
}

/** Over a page's panel: the way back to the app nav (the panel itself is the slot below it). */
const TRAIL_LENGTH = 20

/** The page's name for "Back to …": its nav item, or Settings / Status (account menu, footer row); null when it has none. */
function labelFor(path: string, items: readonly AnyNavItem[]): string | null {
  if (moduleOf(path) === 'settings') return copy.nav.settings
  if (path === '/status') return copy.statusPage
  return activeItem(path, items)?.label ?? null
}

/** Over a page's panel: the way back (to the page before it, or the app nav); the panel itself is the slot below. */
function BackRow({
  focusNextRef,
  name,
  onBack,
}: {
  focusNextRef: FocusNext
  /** Where it goes ("Back to TokenOps"): the accessible name and the tooltip; the row itself reads "Back". */
  name: string
  onBack(): void
}) {
  const back = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    if (focusNextRef.current !== 'panel') return
    focusNextRef.current = null
    back.current?.focus()
  }, [focusNextRef])
  return (
    <SidebarGroup className={cn(GROUP, 'py-1')}>
      <SidebarMenu>
        <SidebarMenuItem>
          <SidebarMenuButton
            ref={back}
            onClick={onBack}
            aria-label={name}
            title={name}
            className={cn(ROW, 'text-muted-foreground')}
          >
            <ArrowLeft aria-hidden />
            <span>{copy.back}</span>
          </SidebarMenuButton>
        </SidebarMenuItem>
      </SidebarMenu>
    </SidebarGroup>
  )
}

/** The scrolling middle (§4.2a): header and footer stay pinned; a soft fade marks hidden items. */
function NavGroups({
  pathname,
  badges,
  hidden,
  focusNextRef,
  onPick,
}: {
  pathname: string
  badges?: NavBadges
  hidden?: readonly string[]
  focusNextRef: FocusNext
  onPick(): void
}) {
  // A layer's nav patches (EE: Lab → Weave fixtures) apply on top of the core's items.
  const items = applyNav(NAV_ITEMS, use(EditionContext).edition.layers).filter(
    (i) => !hidden?.includes(i.to),
  )
  const active = activeItem(pathname, items)
  const { setOpenMobile } = useSidebar()
  const scroller = useRef<HTMLDivElement>(null)
  // Back from a panel: focus the current item (the Main menu button it replaces is gone).
  useEffect(() => {
    if (focusNextRef.current !== 'menu') return
    focusNextRef.current = null
    const el = scroller.current
    const current = el?.querySelector<HTMLElement>('[aria-current="page"]')
    const target = current ?? el?.querySelector('a')
    target?.focus()
  }, [focusNextRef])
  const [edges, setEdges] = useState({ top: false, bottom: false })
  const measure = useCallback(() => {
    const el = scroller.current
    if (!el) return
    const top = el.scrollTop > 0
    const bottom = el.scrollTop + el.clientHeight < el.scrollHeight - 1
    // Unchanged edges: keep the same object so scrolling doesn't re-render every row.
    setEdges((prev) => (prev.top === top && prev.bottom === bottom ? prev : { top, bottom }))
  }, [])
  // Measure on mount and whenever the window or the rail changes size, not only on scroll.
  useLayoutEffect(() => {
    measure()
    const el = scroller.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [measure])
  return (
    <nav aria-label={copy.navLabel} className="flex min-h-0 flex-1 flex-col">
      <SidebarContent
        ref={scroller}
        onScroll={measure}
        className={cn(
          // py-1 leaves room for the first and last rows' focus ring; the rail scrolls too (§4.2a).
          '[scrollbar-width:thin] [scrollbar-color:var(--border)_transparent] gap-0 py-1 group-data-[collapsible=icon]:overflow-auto',
          edges.top && edges.bottom
            ? '[mask-image:linear-gradient(to_bottom,transparent,black_16px,black_calc(100%-16px),transparent)]'
            : edges.top
              ? '[mask-image:linear-gradient(to_bottom,transparent,black_16px)]'
              : edges.bottom
                ? '[mask-image:linear-gradient(to_top,transparent,black_16px)]'
                : undefined,
        )}
      >
        {/* A group with no items (Lab, in a production build) has no header either. */}
        {NAV_GROUPS.filter((g) => items.some((n) => n.group === g.id)).map((g) => (
          <SidebarGroup key={g.id} className={cn(GROUP, 'py-0')}>
            {/* In the rail a header becomes a short hairline: the icons stay one even column, the groups still apart. */}
            {g.label ? (
              <>
                <SidebarGroupLabel className="mt-5 mb-1 h-4 px-2 text-2xs font-medium tracking-[0.04em] text-muted-foreground uppercase group-data-[collapsible=icon]:hidden">
                  {g.label}
                </SidebarGroupLabel>
                <div
                  aria-hidden
                  className="mx-auto my-2 hidden h-px w-5 bg-sidebar-border group-data-[collapsible=icon]:block"
                />
              </>
            ) : null}
            <SidebarMenu>
              {items
                .filter((n) => n.group === g.id)
                .map((n) => (
                  <SidebarMenuItem key={n.to}>
                    <SidebarMenuButton
                      asChild
                      isActive={active?.to === n.to}
                      tooltip={n.label}
                      className={cn(ROW, ACTIVE_ROW)}
                    >
                      <Link
                        to={n.to}
                        search={
                          n.shared
                            ? (prev: Record<string, unknown>) =>
                                // Sessions (7d) and TokenOps (30d) start on their own window, whatever the last page had.
                                n.to === '/sessions' || n.to === '/tokenops'
                                  ? withoutWindow(prev)
                                  : pickShared(prev)
                            : undefined
                        }
                        // `/` is a prefix of every path: Overview is active only on `/` itself.
                        activeOptions={{ includeSearch: false, exact: n.to === '/' }}
                        // The Link only knows its own path; Agents is current on Deploy and Builds too (`also`).
                        aria-current={active?.to === n.to ? 'page' : undefined}
                        // Same-path links (the current page) don't change the pathname: close the sheet here too.
                        onClick={() => {
                          onPick()
                          setOpenMobile(false)
                        }}
                      >
                        <n.icon aria-hidden />
                        <span className={LABEL}>{n.label}</span>
                        {badges?.[n.to] ? (
                          <span className="sr-only">, {badges[n.to]?.label}</span>
                        ) : null}
                      </Link>
                    </SidebarMenuButton>
                    {/* e.g. on Agents: builds started this session and still running (plans/feat-deploy.md §5, design review 8). */}
                    {badges?.[n.to] ? (
                      <SidebarMenuBadge aria-hidden data-testid={`nav-count-${n.to.slice(1)}`}>
                        {badges[n.to]?.count}
                      </SidebarMenuBadge>
                    ) : null}
                  </SidebarMenuItem>
                ))}
            </SidebarMenu>
          </SidebarGroup>
        ))}
      </SidebarContent>
    </nav>
  )
}

/** Connection state plus the mock/live badge; links to the Status page (§3 rule 6). */
function StatusRow() {
  // On every page and in every tab: a slower check, and none on window focus (review, performance).
  const health = useHealth({
    refetchInterval: SIDEBAR_HEALTH_INTERVAL_MS,
    refetchOnWindowFocus: false,
  })
  const [slow, setSlow] = useState(false)
  useEffect(() => {
    if (!health.isPending) return
    const t = setTimeout(() => setSlow(true), CHECKING_DELAY_MS)
    return () => clearTimeout(t)
  }, [health.isPending])
  const state = health.isError
    ? 'unreachable'
    : health.isSuccess
      ? 'connected'
      : slow
        ? 'checking'
        : 'quiet'
  const text =
    state === 'unreachable'
      ? copy.status.unreachable
      : state === 'connected'
        ? copy.status.connected
        : state === 'checking'
          ? copy.status.checking
          : ''
  const mock = env.mode === 'mock'
  const badge = mock ? copy.status.mockBadge : copy.status.liveBadge
  const badgeTitle = mock ? copy.status.mockTitle : copy.status.liveTitle(env.partialMocks)
  const tooltip = [state === 'unreachable' ? copy.status.unreachableHint : text, badge]
    .filter(Boolean)
    .join(' · ')
  return (
    <SidebarMenuItem>
      <SidebarMenuButton asChild tooltip={tooltip} className={cn(ROW, 'relative')}>
        <Link
          to="/status"
          activeOptions={{ exact: true, includeSearch: false }}
          data-testid="status-row"
          data-state={state}
        >
          <span className="relative flex size-4 shrink-0 items-center justify-center" aria-hidden>
            <span
              className={cn(
                'size-2 rounded-full',
                state === 'connected'
                  ? 'bg-success'
                  : state === 'unreachable'
                    ? 'bg-destructive'
                    : 'bg-muted-foreground/50',
              )}
            />
            {state === 'unreachable' ? (
              <CircleAlert className="absolute -top-1 -right-1.5 size-2.5! text-destructive" />
            ) : null}
          </span>
          <span className={cn(LABEL, 'flex min-w-0 flex-1 items-center gap-2')}>
            {state === 'unreachable' ? (
              <span className="truncate">
                <span className="sr-only">{copy.status.unreachable}</span>
                <span aria-hidden>{copy.status.unreachableShort}</span>
              </span>
            ) : (
              <span className="truncate">{text}</span>
            )}
            <span
              title={badgeTitle}
              className={cn(
                'ml-auto shrink-0 rounded-sm border px-1 text-3xs leading-4 font-medium',
                mock ? 'border-warning/50 text-warning' : 'border-border text-muted-foreground',
              )}
            >
              {badge}
            </span>
          </span>
          {/* The rail's one-letter mode badge; the label (with the full badge) is hidden there. */}
          <span
            aria-hidden
            className={cn(
              'absolute right-0.5 bottom-0.5 hidden text-4xs leading-none font-semibold group-data-[collapsible=icon]:block',
              mock ? 'text-warning' : 'text-muted-foreground',
            )}
          >
            {mock ? copy.status.mockShort : copy.status.liveShort}
          </span>
        </Link>
      </SidebarMenuButton>
    </SidebarMenuItem>
  )
}

/**
 * The signed-in user, or "Account unavailable" when /api/me failed with a non-401 (eng D7). While `me`
 * reloads (the cache was reset under a mounted shell) the row says "Loading account…": not an
 * error, and Sign out is disabled until `me` loads, because signing out without a known user would
 * clear every user's drafts (review, red team).
 */
function AccountMenu({ onPick }: { onPick(): void }) {
  // The _app guard already loaded `me`; this only reads it (and refetches on Retry).
  const me = useQuery({ ...meQuery, refetchOnMount: false, retryOnMount: false })
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const router = useRouter()
  const { isMobile, setOpenMobile } = useSidebar()
  // Picking Settings moves focus into its panel (BackRow), not back to the menu's trigger.
  const navigated = useRef(false)
  const [busy, setBusy] = useState(false)
  const name = me.data?.username
  const loading = !name && !me.isError
  const label = name ?? (loading ? copy.account.loading : copy.account.unavailable)
  const run = async () => {
    if (busy) return
    setBusy(true)
    try {
      await signOut({ queryClient, userId: me.data?.sub, navigate: (to) => navigate(to) })
    } finally {
      setBusy(false)
    }
  }
  return (
    <SidebarMenuItem>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <SidebarMenuButton
            tooltip={label}
            aria-label={name ? copy.account.menu(name) : label}
            className={ROW}
          >
            {name ? (
              <CircleUser aria-hidden />
            ) : loading ? (
              <CircleUser aria-hidden className="text-muted-foreground" />
            ) : (
              <CircleAlert aria-hidden className="text-warning" />
            )}
            <span className={cn(LABEL, loading && 'text-muted-foreground')}>{label}</span>
          </SidebarMenuButton>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          side={isMobile ? 'top' : 'right'}
          align="end"
          sideOffset={16}
          className="w-48"
          onCloseAutoFocus={(e) => {
            if (!navigated.current) return
            navigated.current = false
            // The panel swaps in only now: mounted under the open menu, its focus would be pulled back into the menu.
            e.preventDefault()
            onPick()
          }}
        >
          {/* Says why Sign out is greyed out, instead of a menu with nothing to do. */}
          {loading ? (
            <DropdownMenuLabel className="text-xs font-normal text-muted-foreground">
              {copy.account.loadingHint}
            </DropdownMenuLabel>
          ) : null}
          {me.isError && !name ? (
            <DropdownMenuItem
              onSelect={() =>
                void me.refetch().then((r) => {
                  // The guard ignores a first-load 401 on `me`; here it means the session is gone.
                  if (r.error instanceof ApiError && r.error.status === 401) {
                    // The router's location, not window.location: they differ under memory history (tests,
                    // embeds). Read on click, so the menu doesn't re-render on every URL change.
                    void navigate({
                      to: '/login',
                      search: { redirect: router.state.location.href, expired: true },
                    })
                  }
                })
              }
            >
              <RotateCw aria-hidden /> {copy.account.retry}
            </DropdownMenuItem>
          ) : null}
          {/* The quick switch; Settings → Account → Appearance has the same choices with previews. */}
          <ThemeSubmenu />
          <DropdownMenuSeparator />
          {/* Change password lives in Settings → Account. */}
          <DropdownMenuItem asChild>
            <Link
              to="/settings"
              onClick={() => {
                navigated.current = true
                setOpenMobile(false)
              }}
            >
              <Settings aria-hidden /> {copy.account.settings}
            </Link>
          </DropdownMenuItem>
          <DropdownMenuItem
            disabled={busy || loading}
            onSelect={(e) => {
              // Keep the menu open so "Signing out…" is visible until the page changes.
              e.preventDefault()
              void run()
            }}
          >
            <LogOut aria-hidden /> {busy ? copy.account.signingOut : copy.account.signOut}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </SidebarMenuItem>
  )
}

/** Mode and colour theme as radio groups; picking one keeps the menu open, so both can be set in one visit. */
function ThemeSubmenu() {
  const prefs = useThemePrefs()
  const id = useId()
  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger>
        <Palette aria-hidden /> {copy.theme.menu}
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent className="w-48">
        <DropdownMenuLabel id={`${id}-mode`} className="text-xs text-muted-foreground">
          {copy.theme.modeGroup}
        </DropdownMenuLabel>
        <DropdownMenuRadioGroup
          aria-labelledby={`${id}-mode`}
          value={prefs.theme}
          onValueChange={(v) => setTheme(v as Theme)}
        >
          {THEMES.map((t) => (
            <DropdownMenuRadioItem key={t.id} value={t.id} onSelect={(e) => e.preventDefault()}>
              {t.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuLabel id={`${id}-accent`} className="text-xs text-muted-foreground">
          {copy.theme.accentGroup}
        </DropdownMenuLabel>
        <DropdownMenuRadioGroup
          aria-labelledby={`${id}-accent`}
          value={prefs.accent}
          onValueChange={(v) => setAccent(v as Accent)}
        >
          {ACCENTS.map((a) => (
            <DropdownMenuRadioItem key={a.id} value={a.id} onSelect={(e) => e.preventDefault()}>
              <span
                aria-hidden
                className="size-3 shrink-0 rounded-full border border-border"
                style={{ background: a.swatch }}
              />
              {a.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  )
}

const IS_MAC = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform)

/** Top right of the header; in the rail, under the mark. Phones open the sheet from the top bar instead. */
/** Top right of the open sidebar's header. Phones open the sheet from the top bar instead. */
function CollapseButton() {
  const { toggleSidebar, isMobile } = useSidebar()
  if (isMobile) return null
  const label = `${copy.collapse} (${copy.collapseShortcut(IS_MAC)})`
  return (
    <SidebarMenuItem>
      <SidebarMenuButton
        onClick={toggleSidebar}
        // Tooltips only render in the rail: the open sidebar's icon-only button says it on hover.
        title={label}
        aria-label={copy.collapse}
        aria-keyshortcuts={IS_MAC ? 'Meta+B' : 'Control+B'}
        className={cn(ROW, 'w-8 justify-center text-muted-foreground')}
      >
        <PanelLeftClose aria-hidden />
      </SidebarMenuButton>
    </SidebarMenuItem>
  )
}

/** The rail's header: the mark, swapped for the expand icon on hover or keyboard focus; a click expands. */
function ExpandMark() {
  const { toggleSidebar } = useSidebar()
  return (
    <SidebarMenuItem>
      <SidebarMenuButton
        onClick={toggleSidebar}
        tooltip={`${copy.expand} (${copy.collapseShortcut(IS_MAC)})`}
        aria-label={copy.expand}
        aria-keyshortcuts={IS_MAC ? 'Meta+B' : 'Control+B'}
        className={cn(ROW, 'group/expand')}
      >
        <span aria-hidden className="relative size-4 shrink-0">
          <NasikoMark className="size-4 text-logo transition-opacity duration-150 group-hover/expand:opacity-0 group-focus-visible/expand:opacity-0 motion-reduce:transition-none" />
          <PanelLeftOpen className="absolute inset-0 size-4 text-muted-foreground opacity-0 transition-opacity duration-150 group-hover/expand:opacity-100 group-focus-visible/expand:opacity-100 motion-reduce:transition-none" />
        </span>
      </SidebarMenuButton>
    </SidebarMenuItem>
  )
}
