// Ported from nasiko-ui-lab@1b99bdda common/src/lib/useNow.ts. Local edits: `"use client"`; Prettier formatting.
"use client";
import { useEffect, useState } from "react";

/** The current time, re-read every `intervalMs`; `null` stops the clock (the last value stays). */
export function useNow(intervalMs: number | null): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (intervalMs === null) return;
    const id = setInterval(() => {
      setNow(Date.now());
    }, intervalMs);
    return () => {
      clearInterval(id);
    };
  }, [intervalMs]);
  return now;
}
