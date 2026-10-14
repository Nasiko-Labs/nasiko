"use client";

import { useSearchParams } from "@nasiko/app-core/navigation";
import { useCallback, type ReactNode } from "react";
import type { MotionPreference, MotionProfile } from "@nasiko/contracts";
import { MotionProvider, useMotion, type MotionTraceEvent } from "@nasiko/ui/motion";

/**
 * Harness-only motion host. Reads `?profile=&preference=` so test:motion can
 * drive every combination without product UI, and records every recipe event
 * on `window.__motionTrace` so the test asserts what actually animated — not
 * a screenshot of a settled frame (§8.6 "inspect running motion").
 */
declare global {
  interface Window {
    __motionTrace?: MotionTraceEvent[];
  }
}

function pick<T extends string>(v: string | null, allowed: readonly T[], fallback: T): T {
  return allowed.includes(v as T) ? (v as T) : fallback;
}

export function MotionHarness({ children }: { children: ReactNode }) {
  const sp = useSearchParams();
  const profile = pick<MotionProfile>(
    sp.get("profile"),
    ["operational", "expressive", "none"],
    "operational",
  );
  const preference = pick<MotionPreference>(
    sp.get("preference"),
    ["system", "reduced", "off"],
    "system",
  );
  // `?ceiling=reduced|off` wraps one more surface around the host to exercise
  // the parent clamp: a surface can never resolve above its parent. The host
  // itself is a surface under the app's UiProvider, the single owner of the
  // application preference and of `html[data-motion]`.
  const ceiling = pick<MotionPreference>(sp.get("ceiling"), ["system", "reduced", "off"], "system");
  const enter = sp.get("restored") !== "1";
  const trace = useCallback((e: MotionTraceEvent) => {
    (window.__motionTrace ??= []).push(e);
  }, []);
  const surface = (
    <MotionProvider profile={profile} preference={preference} enter={enter} trace={trace}>
      <Readout />
      {children}
    </MotionProvider>
  );
  return ceiling === "system" ? (
    surface
  ) : (
    <MotionProvider preference={ceiling}>{surface}</MotionProvider>
  );
}

function Readout() {
  const m = useMotion();
  return (
    <p
      className="mb-3 text-xs text-muted-foreground"
      data-testid="policy"
      data-mode={m.mode}
      data-profile={m.profile}
      data-os-reduced={String(m.osReduced)}
    >
      mode={m.mode} profile={m.profile} preference={m.preference} os-reduced={String(m.osReduced)}
    </p>
  );
}
