/** Keyboard rules shared by the shell and pages. */

/** ⌘ rather than Ctrl for the modifier shortcuts. */
export const IS_MAC = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform)

/** True while the user is typing: a letter shortcut must not steal keys from a field. */
export function isTypingTarget(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable || target.closest('input, textarea, select') !== null)
  )
}

/** Widgets whose letters are their own (menu, menubar, listbox and combobox typeahead). */
const OWN_KEYS = '[role="menu"], [role="menubar"], [role="listbox"], [role="combobox"]'

/**
 * J (next) and K (previous) for a Previous/Next control: 1, -1, or 0 to ignore. Caps Lock is fine; Shift is a
 * modifier. Ignored with a modifier, on key repeat, during IME composition, when already handled
 * (defaultPrevented), while typing, inside a menu, menubar, listbox or combobox (their typeahead), inside a
 * dialog the control isn't in, and when the control is hidden behind a modal (aria-hidden or inert). So a page's
 * control stays still behind a sheet, while a sheet's own control works inside it.
 */
export function stepFromKey(e: KeyboardEvent, control: Element | null): -1 | 0 | 1 {
  const key = e.key.toLowerCase()
  if ((key !== 'j' && key !== 'k') || e.shiftKey) return 0
  if (e.metaKey || e.ctrlKey || e.altKey || e.repeat || e.isComposing || e.defaultPrevented)
    return 0
  if (isTypingTarget(e.target)) return 0
  const target = e.target instanceof Element ? e.target : null
  if (target?.closest(OWN_KEYS)) return 0
  const dialog = target?.closest('[role="dialog"], [role="alertdialog"]')
  if (dialog && !(control && dialog.contains(control))) return 0
  // Radix marks everything outside an open modal aria-hidden (some set inert), and focus can fall to <body>.
  if (control?.closest('[aria-hidden="true"], [inert]')) return 0
  return key === 'j' ? 1 : -1
}
