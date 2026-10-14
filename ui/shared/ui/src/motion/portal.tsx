"use client";

import { createContext, useContext } from "react";

/**
 * Where portalled overlays (dialog, sheet, popover, menu, select, tooltip)
 * mount. `null` means `document.body` — the application scope, whose motion
 * policy is published on `<html>`. A surface-scope MotionProvider provides its
 * own container element (rendered inside its `data-motion` wrapper), so the
 * overlays a surface opens inherit *that surface's* effective policy through
 * CSS, while the application/OS ceiling still applies because every surface
 * policy is clamped by its parent at resolution time.
 */
export const PortalContainerContext = createContext<HTMLElement | null>(null);

/**
 * The container the vendored primitives pass to their `Portal` (patched in; see
 * PATCHES.json). `undefined` — not `null` — when there is no surface container,
 * so Base UI falls back to `document.body` instead of treating it as "nowhere".
 */
export function usePortalContainer(): HTMLElement | undefined {
  return useContext(PortalContainerContext) ?? undefined;
}
