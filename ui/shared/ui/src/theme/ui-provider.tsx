"use client";

import type { ReactNode } from "react";
import { MotionProvider, type MotionProviderProps } from "../motion/provider";
import { DensityProvider, type Density } from "./density";
import { ThemeProvider } from "./theme-provider";
import type { ForcedTheme } from "./theme-store";

/**
 * The one root provider an app mounts: theme → density → motion. Everything
 * below reads contracts from here; nothing else creates a theme or a second
 * motion runtime (Phase 3 exit: "no duplicate theme"). The theme is the lab's
 * store (`theme-store.ts`); `nonce` is passed through for the app's inline
 * pre-paint script, `forcedTheme` shows one mode for stories and tests.
 *
 * Deliberately *not* here: `TooltipProvider`. Base UI tooltips work without it
 * (it only shares hover delay across a group), and mounting it at the root put
 * the floating-ui positioning engine (~38 KiB Brotli) on every route including
 * the neutral one. Screens that want grouped delays wrap their own region.
 */
export interface UiProviderProps {
  readonly children: ReactNode;
  readonly density?: Density;
  readonly motion?: Omit<MotionProviderProps, "children">;
  readonly nonce?: string;
  readonly forcedTheme?: ForcedTheme;
}

export function UiProvider({
  children,
  density = "comfortable",
  motion,
  nonce,
  forcedTheme,
}: UiProviderProps) {
  return (
    <ThemeProvider {...(nonce ? { nonce } : {})} {...(forcedTheme ? { forcedTheme } : {})}>
      <DensityProvider density={density}>
        <MotionProvider scope="app" {...motion}>
          {children}
        </MotionProvider>
      </DensityProvider>
    </ThemeProvider>
  );
}
