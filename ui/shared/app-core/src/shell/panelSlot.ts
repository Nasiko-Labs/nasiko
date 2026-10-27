/**
 * The sidebar's drill-in slot (Chat's history, Settings' sections): a page renders `SidebarPanel`, and while the
 * expanded sidebar (or the phone sheet) can hold it, the panel replaces the app nav there, with a way back. The shell
 * never imports the page: the page portals into the element the sidebar hands out here.
 */
import { createContext, type Dispatch, type SetStateAction } from 'react'

export interface SidebarPanelSlot {
  /** Where a panel renders; null while the sidebar shows the app nav (collapsed rail, closed sheet). */
  target: HTMLElement | null
  setTarget: (el: HTMLElement | null) => void
  /** How many panels are mounted: the sidebar drills in while one is. */
  panels: number
  setPanels: Dispatch<SetStateAction<number>>
}

export const SidebarPanelContext = createContext<SidebarPanelSlot>({
  target: null,
  setTarget: () => {},
  panels: 0,
  setPanels: () => {},
})
