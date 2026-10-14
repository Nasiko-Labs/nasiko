"use client";

import { createContext, useContext, type ReactNode } from "react";

export type Density = "comfortable" | "compact";
const DensityContext = createContext<Density>("comfortable");

/**
 * Density scales control height, row padding and gaps through the
 * `--density-*` tokens (tokens.css). It is a *contract* in Phase 3: no
 * preference UI exists yet; screens read it, tests set it.
 */
export function DensityProvider({ density, children }: { density: Density; children: ReactNode }) {
  return (
    <DensityContext.Provider value={density}>
      <div data-density={density} style={{ display: "contents" }}>
        {children}
      </div>
    </DensityContext.Provider>
  );
}

export function useDensity(): Density {
  return useContext(DensityContext);
}
