/**
 * The sidebar's items (plans/feat-app-shell.md §3): one source for AppSidebar and its tests.
 * Only pages that exist get an item. Overview is `/`; Status (`/status`) is reached from the footer's status row,
 * and Settings is a footer row too. Deploy and Builds belong to Agents (its sub-nav), so they mark it current.
 *
 * `shared` items carry the shared context (window, filters, compare) between them through
 * `pickShared` (src/app/shell/context.ts), as the old header nav did.
 */
import {
  Bot,
  DollarSign,
  LayoutDashboard,
  ListTree,
  MessageSquare,
  Plug,
  SquareTerminal,
  Waypoints,
  Workflow,
  type LucideIcon,
} from 'lucide-react'
import type { AnyNavItem } from '../edition'
import { copy } from './copy'

export type NavGroupId = 'work' | 'fleet' | 'connect' | 'observe' | 'lab'

/** In display order, grouped by job; the first group has no header. */
export const NAV_GROUPS: readonly { id: NavGroupId; label: string | null }[] = [
  { id: 'work', label: null },
  { id: 'fleet', label: copy.nav.fleet },
  { id: 'connect', label: copy.nav.connect },
  { id: 'observe', label: copy.nav.observe },
  { id: 'lab', label: copy.nav.lab },
]

export interface NavItem {
  to:
    | '/'
    | '/chat'
    | '/agents'
    | '/router'
    | '/mcp'
    | '/workflows'
    | '/sessions'
    | '/tokenops'
    | '/harnesses'
  label: string
  icon: LucideIcon
  group: NavGroupId
  shared: boolean
  /** Other pages (and their sub-routes) that belong to this item and mark it current. */
  also?: readonly string[]
}

export const NAV_ITEMS: readonly NavItem[] = [
  { to: '/', label: copy.nav.overview, icon: LayoutDashboard, group: 'work', shared: false },
  { to: '/chat', label: copy.nav.chat, icon: MessageSquare, group: 'work', shared: false },
  // plans/feat-deploy.md §3: Deploy and Builds are Agents' sub-pages (its sub-nav and "Deploy an agent").
  {
    to: '/agents',
    label: copy.nav.agents,
    icon: Bot,
    group: 'fleet',
    shared: false,
    also: ['/deploy', '/builds'],
  },
  // plans/feat-workflows.md §1: Drafts and Runs are its sub-pages (/workflows/*).
  { to: '/workflows', label: copy.nav.workflows, icon: Workflow, group: 'fleet', shared: false },
  // What agents can call: models (plans/feat-llm-router.md §4), then tools (plans/feat-mcp.md §1).
  { to: '/router', label: copy.nav.router, icon: Waypoints, group: 'connect', shared: false },
  { to: '/mcp', label: copy.nav.mcp, icon: Plug, group: 'connect', shared: false },
  { to: '/sessions', label: copy.nav.sessions, icon: ListTree, group: 'observe', shared: true },
  { to: '/tokenops', label: copy.nav.tokenops, icon: DollarSign, group: 'observe', shared: true },
  {
    to: '/harnesses',
    label: copy.nav.harnesses,
    icon: SquareTerminal,
    group: 'observe',
    shared: true,
  },
]

const under = (pathname: string, to: string) =>
  pathname === to || (to !== '/' && pathname.startsWith(`${to}/`))

/**
 * The item for a pathname, including its sub-routes (`/agents/mine`, `/chat/<id>`) and the pages it owns (`/builds/<id>`
 * is Agents); Overview only on `/` itself, none on `/status` or `/settings` (footer rows).
 */
export function activeItem(
  pathname: string,
  items: readonly AnyNavItem[] = NAV_ITEMS,
): AnyNavItem | undefined {
  return items.find((n) => [n.to, ...(n.also ?? [])].some((to) => under(pathname, to)))
}

export const pathOf = (href: string) => href.split(/[?#]/)[0] ?? href
/** The module a path belongs to (`/chat/abc` → `chat`, `/` → ''): moves inside it don't count as "where you came from". */
export const moduleOf = (path: string) => path.split('/')[1] ?? ''

/**
 * The drill-in panel's Back target: the index in `trail` (visited hrefs, oldest first, with their search) of the newest
 * page outside the current page's module, so another chat or another Settings section never counts; -1 when the user
 * started here. Back drops the trail from that index on, like history, so Chat → Settings → Back → Back keeps going back.
 */
export function backIndex(trail: readonly string[], pathname: string): number {
  const here = moduleOf(pathname)
  return trail.findLastIndex((h) => moduleOf(pathOf(h)) !== here)
}
