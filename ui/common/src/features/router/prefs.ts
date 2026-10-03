/**
 * Per-viewer disclosure choices on the router page ("How routing works", the agents on your default). Storage can be
 * blocked or empty (private windows, previews): a read then says "no choice yet" and the page uses its default.
 */
export const PREF_HOW = 'openruntime.router.howItWorks'
export const PREF_DEFAULTS = 'openruntime.router.defaultsOpen'

export function readOpen(key: string): boolean | null {
  try {
    const v = globalThis.localStorage?.getItem(key)
    return v === null || v === undefined ? null : v === 'open'
  } catch {
    return null
  }
}

export function writeOpen(key: string, open: boolean) {
  try {
    globalThis.localStorage?.setItem(key, open ? 'open' : 'closed')
  } catch {
    /* storage blocked: the choice lasts for this visit */
  }
}
