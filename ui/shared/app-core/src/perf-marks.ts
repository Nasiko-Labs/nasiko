/**
 * Performance marks used by the harness journeys (contract §16.1). Names are
 * stable identifiers; measurements are read by tests/e2e via
 * performance.getEntriesByName. No-ops where the API is unavailable (SSR).
 */
export const PERF_MARKS = {
  shellVisible: "nasiko:shell-visible",
  firstUsefulData: "nasiko:first-useful-data",
  surfaceValidated: "nasiko:surface-validated",
  surfacePainted: "nasiko:surface-painted",
} as const;

export type PerfMarkName = (typeof PERF_MARKS)[keyof typeof PERF_MARKS];

export function mark(name: PerfMarkName): void {
  if (typeof performance === "undefined" || typeof performance.mark !== "function") return;
  performance.mark(name);
}

export function measure(name: string, start: PerfMarkName, end: PerfMarkName): number | undefined {
  if (typeof performance === "undefined" || typeof performance.measure !== "function")
    return undefined;
  try {
    const m = performance.measure(name, start, end);
    return m.duration;
  } catch {
    return undefined;
  }
}
