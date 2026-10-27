import { use, useLayoutEffect, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { SidebarPanelContext } from './panelSlot'

/**
 * A page's own navigation (`panel`), shown in the sidebar in place of the app nav: one sidebar, never two side by side.
 * `children` is the page, told whether the panel is in the sidebar; where it isn't (the collapsed rail, a closed phone
 * sheet), the page shows its own fallback. Keep the page's content at the same place in its tree either way, so a
 * collapse or expand never remounts it.
 */
export function SidebarPanel({
  panel,
  children,
}: {
  panel: ReactNode
  children: (inSidebar: boolean) => ReactNode
}) {
  const { target, setPanels } = use(SidebarPanelContext)
  useLayoutEffect(() => {
    setPanels((n) => n + 1)
    return () => setPanels((n) => n - 1)
  }, [setPanels])
  return (
    <>
      {target ? createPortal(panel, target) : null}
      {children(target !== null)}
    </>
  )
}
