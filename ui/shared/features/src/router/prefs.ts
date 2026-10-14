// Ported from nasiko-ui-lab@1b99bdda common/src/features/router/prefs.ts. Local edits: the storage guards carry lint exemptions (the
// types say `localStorage` always exists; it did not while Next rendered on the server, before the SPA); Prettier formatting.
/**
 * Per-viewer disclosure choices on the router page ("How routing works", the agents on your default). Storage can be
 * blocked or empty (private windows, previews): a read then says "no choice yet" and the page uses its default.
 */
export const PREF_HOW = "openruntime.router.howItWorks";
export const PREF_DEFAULTS = "openruntime.router.defaultsOpen";

export function readOpen(key: string): boolean | null {
  try {
    // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- kept from the Next port (absent during a server render); always present in the SPA
    const v = globalThis.localStorage?.getItem(key);
    // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- undefined when storage is absent (above)
    return v === null || v === undefined ? null : v === "open";
  } catch {
    return null;
  }
}

export function writeOpen(key: string, open: boolean) {
  try {
    // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- kept from the Next port (absent during a server render); always present in the SPA
    globalThis.localStorage?.setItem(key, open ? "open" : "closed");
  } catch {
    /* storage blocked: the choice lasts for this visit */
  }
}
