// Destination: not in the lab. The kit's one reduced-motion decision, replacing its direct `useReducedMotion()`
// calls (each call site's header says so). The lab reads only the OS preference; this repository's motion host
// (`../motion/provider.tsx`, ADR-0006) also clamps by surface profile, user preference and parent scope, and a
// chart inside a `MotionProvider` whose resolved mode is `none` or `reduced` must take the kit's own at-rest path —
// the one the lab already draws for OS reduced motion. Outside any provider the lab's rule is kept unchanged.
"use client";

import { useReducedMotion } from "motion/react";
import { useMotionScope } from "../motion/provider";

/** `true` when the kit must draw at rest: the OS asks for reduced motion, or the enclosing motion host's mode is not `full`. */
export function useKitReducedMotion(): boolean {
  const os = useReducedMotion() === true;
  const scope = useMotionScope();
  return os || (scope !== null && scope.mode !== "full");
}
