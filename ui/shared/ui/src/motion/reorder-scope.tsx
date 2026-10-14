// No lab counterpart: the lab's app ran a non-strict `LazyMotion`, so `Reorder.Group`/`Reorder.Item` (which render the
// eager `motion.*` components internally) worked anywhere. Local edits: n/a (new file). The destination's app root is
// strict (provider.tsx: an eager `motion` component under it throws), so a drag-to-reorder list renders inside this
// scope: a nested, non-strict `LazyMotion` with the `domMax` bundle (drag and layout, ./features-max.ts) that lets
// those eager components mount. Everything outside the scope stays strict; the policy (`MotionConfig`) is inherited.
"use client";

import { LazyMotion } from "motion/react";
import type { ReactNode } from "react";

const loadMaxFeatures = () => import("./features-max").then((m) => m.default);

/** Wrap a `Reorder.Group` (and nothing else) in this. */
export function ReorderScope({ children }: { children: ReactNode }) {
  return <LazyMotion features={loadMaxFeatures}>{children}</LazyMotion>;
}
