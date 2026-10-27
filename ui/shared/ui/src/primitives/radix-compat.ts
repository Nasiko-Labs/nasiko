/**
 * A local edit on the vendored Base UI primitives (stage D), so the pages keep the Radix-era props they pass and
 * behave as they did: an auto-focus handler (`onOpenAutoFocus`/`onCloseAutoFocus`, which calls `preventDefault()` to
 * keep focus where it put it) becomes Base UI's `initialFocus`/`finalFocus`. The handler gets a cancelable Event.
 */
export type AutoFocusHandler = (event: Event) => void

export function focusProp(handler: AutoFocusHandler | undefined) {
  if (!handler) return undefined
  return () => {
    const event = new Event("autofocus", { cancelable: true })
    handler(event)
    // true: Base UI focuses as it would by default; false: the handler moved focus itself.
    return !event.defaultPrevented
  }
}
