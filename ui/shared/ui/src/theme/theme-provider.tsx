"use client";

import { useEffect, type ReactNode } from "react";
import { forceTheme, initTheme, type ForcedTheme } from "./theme-store";

/**
 * Mode (System / Light / Dark) and colour theme (Carbon / Teal / Indigo / Plum) through the lab's
 * zustand store (`theme-store.ts`): `.dark` and `data-theme` on <html>, which the `dark` custom
 * variant and the `[data-theme]` token blocks in styles/tokens.css key on. The server renders
 * without either; the app's root layout inlines `THEME_PREPAINT_SCRIPT` (under the page CSP
 * nonce) so they are set before first paint, and `initTheme()` takes over here: it re-applies the
 * stored choice (in case the inline script was blocked), follows the OS while the choice is System
 * and picks up choices made in other tabs. Colours never animate on a switch (the store sets
 * `theme-switching` for one frame).
 *
 * `forcedTheme` (stories, tests) shows that mode without storing it. `nonce` is accepted for the
 * provider's callers; the script itself is inlined by the layout, so nothing is rendered here.
 */
export function ThemeProvider({
  children,
  forcedTheme,
}: {
  children: ReactNode;
  nonce?: string;
  forcedTheme?: ForcedTheme;
}) {
  useEffect(() => {
    forceTheme(forcedTheme ?? null);
    return () => {
      forceTheme(null);
    };
  }, [forcedTheme]);
  useEffect(() => initTheme(), []);
  return children;
}
