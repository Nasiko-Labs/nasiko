// Ported from nasiko-ui-lab@1b99bdda common/src/mocks/anchor.ts. Local edits: `isRealDate` inlined from
// common/src/lib/search.ts (the lab's search helpers are not ported); `VITE_NASIKO_SEED_ANCHOR` is
// `VITE_NASIKO_SEED_ANCHOR` here (read by mock-boot.tsx); `static override now()` for `noImplicitOverride`.
/**
 * Pinned demo time (mock mode only): `?anchor=YYYY-MM-DD` (or VITE_NASIKO_SEED_ANCHOR)
 * fixes "now" to 15:00 UTC on that day, for both the seed and the app's clock, so the
 * demo script's figures never drift with the calendar. Loaded only from the mock
 * bootstrap, so none of this ships in a live bundle.
 */

/** A real calendar date: `2026-13-01` and `2026-02-30` are rejected, not coerced (lab `lib/search.ts`). */
export function isRealDate(s: string): boolean {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(s)) return false;
  const d = new Date(`${s}T00:00:00Z`);
  return !Number.isNaN(d.getTime()) && d.toISOString().slice(0, 10) === s;
}

export function readAnchor(search: string, envAnchor: string | undefined): Date | null {
  const raw = new URLSearchParams(search).get("anchor") ?? envAnchor ?? "";
  return isRealDate(raw) ? new Date(`${raw}T15:00:00.000Z`) : null;
}

/**
 * Shift the page clock so `Date.now()` and `new Date()` read as the anchor (plus elapsed
 * time), like the tests' fake timers. Explicit dates are untouched.
 */
export function pinClock(anchor: Date): void {
  const offset = anchor.getTime() - Date.now();
  const RealDate = Date;
  class AnchoredDate extends RealDate {
    constructor(...args: unknown[]) {
      if (args.length === 0) super(RealDate.now() + offset);
      else super(...(args as [string]));
    }
    static override now() {
      return RealDate.now() + offset;
    }
  }
  // `Date()` without `new` returns a string; a class can't be called, so a Proxy handles it.
  globalThis.Date = new Proxy(AnchoredDate, {
    apply: () => new RealDate(RealDate.now() + offset).toString(),
  }) as DateConstructor;
}
