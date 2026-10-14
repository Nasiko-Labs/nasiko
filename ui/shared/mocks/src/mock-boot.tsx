// Ported from nasiko-ui-lab@1b99bdda common/src/app/mount.tsx (the mock-mode boot). Local edits: a client
// component gating the app's children instead of a `mount()` step before `createRoot`; the gate itself is a
// `React.lazy` import inside a branch that folds away outside mock builds; the failure banner lives in mock-gate.tsx; the edition's own handlers
// (the lab's `@edition/mocks`) come from the composition root through `loadEditionHandlers`, never from an import here.
"use client";

import { lazy } from "react";
import type { ReactNode } from "react";
import type { EditionHandlers } from "./handlers";

/**
 * Harness-only mock mode: `VITE_NASIKO_API_MODE=mock` (the `dev:mock` script, or a test-only production
 * build) starts the MSW worker before anything renders, so every `/api/*` call the pilot makes is answered from the
 * seed. `shared/config/vite.config.ts` inlines the variable in every build ("live" unless "mock", and refuses "mock" in a release
 * build), so in any other build `MOCK` is the literal `false`, the `MockGate` branch folds to `null` and the mock
 * chunk (msw, the seed, the handlers) is never emitted (README.md has the check).
 */
const MOCK = import.meta.env.VITE_NASIKO_API_MODE === "mock";

/** `null` when folded out: the dynamic import lives inside the dead branch, so no chunk is made for it. */
const MockGate = MOCK
  ? lazy(() => import("./mock-gate").then((m) => ({ default: m.MockGate })))
  : null;

export interface MockBootProps {
  children: ReactNode;
  /**
   * The edition's own mock handlers, stacked in front of the core's (EE: as nasiko-server-ee answers). The EE app's
   * boot passes a loader that is itself behind its own `VITE_NASIKO_API_MODE === "mock"` literal, so a release
   * build folds the import away as it does this gate (the OSS root's oss/app/src/mock-boot.tsx passes none).
   */
  loadEditionHandlers?: (() => Promise<EditionHandlers>) | undefined;
}

export function MockBoot({ children, loadEditionHandlers }: MockBootProps) {
  return MockGate ? (
    <MockGate loadEditionHandlers={loadEditionHandlers}>{children}</MockGate>
  ) : (
    children
  );
}
