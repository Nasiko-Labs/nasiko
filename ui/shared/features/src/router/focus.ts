import { useCallback, useState } from 'react'

/**
 * Focus back to the opener (plan §4.8). The router's sheets open from plain buttons and menu items, not Radix
 * triggers, so Radix has nothing to return focus to. Remember what had focus when the sheet opened and focus it on
 * close. A menu item is gone by then, so a sheet opened from a row menu returns to that menu's trigger (Radix links it
 * to the menu with `aria-controls`). An opener that was re-rendered elsewhere meanwhile (a routing change moves its row
 * out of the folded group) is found again by its accessible name.
 */
function openerOf(el: Element | null): HTMLElement | null {
  if (!(el instanceof HTMLElement)) return null
  const menu = el.closest('[role="menu"]')
  const trigger = menu?.id
    ? document.querySelector<HTMLElement>(`[aria-controls="${CSS.escape(menu.id)}"]`)
    : null
  return trigger ?? el
}
export function useReturnFocus(open: boolean) {
  const [opener, setOpener] = useState<HTMLElement | null>(null)
  const [wasOpen, setWasOpen] = useState(open)
  if (open !== wasOpen) {
    setWasOpen(open)
    if (open) setOpener(openerOf(document.activeElement))
  }
  return useCallback(
    (e: Event) => {
      const label = opener?.getAttribute('aria-label')
      const target = opener?.isConnected
        ? opener
        : label
          ? document.querySelector<HTMLElement>(`[aria-label="${CSS.escape(label)}"]`)
          : null
      if (target) {
        e.preventDefault()
        target.focus()
      }
    },
    [opener],
  )
}
