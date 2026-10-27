'use client'

import { createContext, useContext } from 'react'

/**
 * Where portalled overlays (dialog, sheet, popover, menu, select, tooltip) mount. Nothing in this tree provides it
 * yet, so every overlay mounts in `document.body`; the vendored primitives read it as their source does (stage D), so
 * a later surface with its own container needs no primitive edits.
 */
export const PortalContainerContext = createContext<HTMLElement | null>(null)

/** `undefined`, not `null`, when there is no container, so Base UI falls back to `document.body`. */
export function usePortalContainer(): HTMLElement | undefined {
  return useContext(PortalContainerContext) ?? undefined
}
