import { Link } from '@tanstack/react-router'
import { CircleHelp } from 'lucide-react'
import { Fragment, use, useLayoutEffect, useState, type ReactNode, type ReactElement } from 'react'
import { createPortal } from 'react-dom'
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbSeparator,
} from '@nasiko/ui/primitives/breadcrumb'
import { Button } from '@nasiko/ui/primitives/button'
import { SidebarTrigger } from '@nasiko/ui/primitives/sidebar'
import { askForTour } from '@nasiko/app-core/tour'
import { useMediaQuery } from '@nasiko/ui/lib/useMediaQuery'
import { cn } from '@nasiko/ui/lib/utils'
import { copy } from './copy'
import { TopBarContext, type TopBarSlot } from './topBarSlot'

/**
 * The shell's bar above every page: the phones' menu button, the breadcrumb, then the page's actions and its
 * Previous/Next at the far right, so the arrows keep one place on every page. Without a page's `TopBar` the
 * breadcrumb is the page's nav group (text) and item, a link once you are below it. `bare` (Overview, New chat) keeps it on phones
 * only, for the menu button.
 */
export function ShellTopBar({
  item,
  pathname,
  bare = false,
  children,
}: {
  item: TopBarSlot['item']
  pathname: string
  bare?: boolean
  /** The page area: it reads the slot. */
  children: ReactNode
}) {
  const [crumbs, setCrumbs] = useState<HTMLElement | null>(null)
  const [actions, setActions] = useState<HTMLElement | null>(null)
  const [stepper, setStepper] = useState<HTMLElement | null>(null)
  const [claims, setClaims] = useState(0)
  return (
    <TopBarContext value={{ crumbs, actions, stepper, claims, setClaims, item }}>
      {/* `@container`: a page's bar contents size by the bar (Chat's chip and totals did by the pane). */}
      <header
        data-slot="topbar"
        className={cn(
          '@container sticky top-0 z-30 flex h-(--topbar-h) shrink-0 items-center gap-2 border-b border-border bg-background px-2 md:px-4',
          bare && 'md:hidden',
        )}
      >
        <SidebarTrigger
          data-tour="app-menu"
          aria-label={copy.openMenu}
          className="size-11 md:hidden"
        />
        <div className="flex min-w-0 flex-1 items-center gap-2">
          {claims === 0 && item ? (
            <Crumbs
              group={item.group}
              items={[pathname === item.to ? item.label : <Link to={item.to}>{item.label}</Link>]}
            />
          ) : null}
          <div ref={setCrumbs} className="contents" />
        </div>
        <div
          data-tour="page-actions"
          ref={setActions}
          className="flex shrink-0 items-center gap-1.5 empty:hidden"
        />
        <TourButton />
        <div
          data-tour="page-stepper"
          ref={setStepper}
          className="flex shrink-0 items-center border-l border-border pl-2 empty:hidden"
        />
      </header>
      {children}
    </TopBarContext>
  )
}

/**
 * A breadcrumb: the nav group first, as text (a group is a heading, not a page); then every item but the last is a
 * link (pass a router `Link`); the last is the current page, plain text (or, when it isn't text, drawn as given:
 * Chat's title with its rename button).
 */
function Crumbs({ group, items }: { group?: string | null; items: ReactNode[] }) {
  const last = items.length - 1
  return (
    <Breadcrumb aria-label="Breadcrumb" className="min-w-0">
      <BreadcrumbList className="flex-nowrap gap-1.5 sm:gap-1.5">
        {group ? (
          <>
            <BreadcrumbItem className="shrink-0">{group}</BreadcrumbItem>
            <BreadcrumbSeparator />
          </>
        ) : null}
        {items.map((it, i) => (
          // eslint-disable-next-line @eslint-react/no-array-index-key -- a breadcrumb's levels are positional, never reordered
          <Fragment key={i}>
            {i > 0 ? <BreadcrumbSeparator /> : null}
            {i < last ? (
              <BreadcrumbItem className="shrink-0">
                {/* A non-last level is a link element (its page's Link): BreadcrumbLink renders as it. */}
                <BreadcrumbLink
                  render={it as ReactElement}
                  className="underline-offset-2 hover:underline"
                />
              </BreadcrumbItem>
            ) : (
              <BreadcrumbItem className="min-w-0">
                {typeof it === 'string' || typeof it === 'number' ? (
                  // Plain text, not shadcn's BreadcrumbPage (role="link" on a non-link).
                  <span aria-current="page" className="truncate font-medium text-foreground">
                    {it}
                  </span>
                ) : (
                  it
                )}
              </BreadcrumbItem>
            )}
          </Fragment>
        ))}
      </BreadcrumbList>
    </Breadcrumb>
  )
}

/**
 * A page's part of the top bar. The breadcrumb is the nav group, `root` (default: the page's nav item, as a link), then `trail`, the
 * levels below it ("Settings › Secrets", "Agents › Builds › 5eed…"): every level but the last a router `Link`.
 * Without `root` or `trail` the shell's own breadcrumb stays. `actions` sit at the right, `stepper` (a `PrevNext`)
 * rightmost. Render one per page, in every state the page has (loading, error), so the arrows never move. Below
 * 640 px the actions render here, in the page, so the bar keeps room for the breadcrumb and the arrows;
 * `pinActions` keeps a page's few small ones (Chat's) in the bar.
 */
export function TopBar({
  root,
  trail = [],
  actions,
  stepper,
  pinActions = false,
}: {
  root?: ReactNode
  trail?: ReactNode[]
  actions?: ReactNode
  stepper?: ReactNode
  pinActions?: boolean
}) {
  const slot = use(TopBarContext)
  const { setClaims, item } = slot
  const wide = useMediaQuery('(min-width: 640px)')
  const inBar = wide || pinActions
  const head = root ?? (item ? <Link to={item.to}>{item.label}</Link> : null)
  const claims = root !== undefined || trail.length > 0
  useLayoutEffect(() => {
    if (!claims) return
    setClaims((n) => n + 1)
    return () => setClaims((n) => n - 1)
  }, [claims, setClaims])
  // The group follows the page's nav item (Account settings has none, so no group either).
  const own = claims ? <Crumbs group={item?.group} items={head ? [head, ...trail] : trail} /> : null
  return (
    <>
      {own && slot.crumbs ? createPortal(own, slot.crumbs) : null}
      {actions && slot.actions && inBar ? createPortal(actions, slot.actions) : null}
      {actions && !inBar ? (
        <div className="flex flex-wrap items-center gap-2">{actions}</div>
      ) : null}
      {stepper && slot.stepper ? createPortal(stepper, slot.stepper) : null}
    </>
  )
}

/**
 * Starts this page's tour (`?` does too): the tour feature answers `askForTour` (lib/tour.ts). In the bar, left of the
 * Previous/Next arrows so they stay rightmost; Overview and New chat, which have no bar on desktop, show their own.
 */
export function TourButton({ className }: { className?: string }) {
  return (
    <Button
      variant="ghost"
      size="icon"
      data-tour="tour-button"
      title={`${copy.tour} (?)`}
      aria-label={copy.tour}
      aria-keyshortcuts="?"
      onClick={() => askForTour()}
      className={cn('size-8 shrink-0 text-muted-foreground pointer-coarse:size-11', className)}
    >
      <CircleHelp aria-hidden />
    </Button>
  )
}
