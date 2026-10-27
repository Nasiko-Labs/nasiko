/**
 * The page's top bar (plans/feat-app-shell.md §4.1): the shell draws one bar above every page, and a page fills its
 * breadcrumb, actions and Previous/Next through `TopBar` (TopBar.tsx), which portals into the elements handed out
 * here, as `SidebarPanel` does for the sidebar. The shell never imports the page.
 */
import { createContext, type Dispatch, type SetStateAction } from 'react'
import type { AnyNavItem } from '../edition/edition'

export interface TopBarSlot {
  crumbs: HTMLElement | null
  actions: HTMLElement | null
  stepper: HTMLElement | null
  /** How many mounted `TopBar`s bring their own breadcrumb: the shell's default one hides while any does. */
  claims: number
  setClaims: Dispatch<SetStateAction<number>>
  /** The nav item the page belongs to (layer patches applied) and its nav group's label, the breadcrumb's start. */
  item: (Pick<AnyNavItem, 'to' | 'label'> & { group: string | null }) | null
}

export const TopBarContext = createContext<TopBarSlot>({
  crumbs: null,
  actions: null,
  stepper: null,
  claims: 0,
  setClaims: () => {},
  item: null,
})
