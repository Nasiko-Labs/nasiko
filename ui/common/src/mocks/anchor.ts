/**
 * Pinned demo time (mock mode only): `?anchor=YYYY-MM-DD` (or VITE_NASIKO_SEED_ANCHOR)
 * fixes "now" to 15:00 UTC on that day, for both the seed and the app's clock, so the
 * demo script's figures never drift with the calendar. Loaded only from the mock
 * bootstrap, so none of this ships in a live bundle.
 */
import { isRealDate } from '@/lib/search'

export function readAnchor(search: string, envAnchor: string | undefined): Date | null {
  const raw = new URLSearchParams(search).get('anchor') ?? envAnchor ?? ''
  return isRealDate(raw) ? new Date(`${raw}T15:00:00.000Z`) : null
}

/**
 * Shift the page clock so `Date.now()` and `new Date()` read as the anchor (plus elapsed
 * time), like the tests' fake timers. Explicit dates are untouched.
 */
export function pinClock(anchor: Date): void {
  const offset = anchor.getTime() - Date.now()
  const RealDate = Date
  class AnchoredDate extends RealDate {
    constructor(...args: unknown[]) {
      if (args.length === 0) super(RealDate.now() + offset)
      else super(...(args as [string]))
    }
    static now() {
      return RealDate.now() + offset
    }
  }
  // `Date()` without `new` returns a string; a class can't be called, so a Proxy handles it.
  globalThis.Date = new Proxy(AnchoredDate, {
    apply: () => new RealDate(RealDate.now() + offset).toString(),
  }) as DateConstructor
}
