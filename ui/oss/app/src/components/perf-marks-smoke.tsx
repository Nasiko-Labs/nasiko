"use client";

import { useSyncExternalStore } from "react";
import { PERF_MARKS, mark, measure } from "@nasiko/app-core/perf-marks";

// Harness-only: proves the mark/measure seam works end to end in a browser.
// The measurement is an external (performance timeline) value, so it is read
// through useSyncExternalStore rather than set from an effect.
let cached: number | undefined;
const listeners = new Set<() => void>();
function subscribe(cb: () => void): () => void {
  listeners.add(cb);
  if (cached === undefined) {
    mark(PERF_MARKS.shellVisible);
    mark(PERF_MARKS.firstUsefulData);
    cached = measure("nasiko:shell-to-data", PERF_MARKS.shellVisible, PERF_MARKS.firstUsefulData);
    queueMicrotask(() => {
      for (const l of listeners) l();
    });
  }
  return () => {
    listeners.delete(cb);
  };
}
const getSnapshot = (): number | undefined => cached;
const getServerSnapshot = (): number | undefined => undefined;

export function PerfMarksSmoke() {
  const ms = useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot);
  return (
    <p data-testid="perf-measure" data-ms={ms ?? ""}>
      shell→data: {ms === undefined ? "pending" : `${ms.toFixed(2)} ms`}
    </p>
  );
}
