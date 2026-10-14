"use client";

import { createContext, useContext } from "react";
import type { ReferenceAreaConfig } from "./reference-area-config";

export interface ReferenceAreaRegistrationContextValue {
  registerReferenceArea: (id: string, config: ReferenceAreaConfig) => void;
  unregisterReferenceArea: (id: string) => void;
}

export const ReferenceAreaRegistrationContext =
  createContext<ReferenceAreaRegistrationContextValue | null>(null);

export function useReferenceAreaRegistration(): ReferenceAreaRegistrationContextValue | null {
  return useContext(ReferenceAreaRegistrationContext);
}
