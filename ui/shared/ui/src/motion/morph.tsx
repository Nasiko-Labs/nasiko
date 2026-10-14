"use client";

import type { ComponentProps } from "react";
import * as m from "motion/react-m";

/**
 * A shared-layout (`layoutId`) element: when two `MorphSpan`s with the same `layoutId` swap (one unmounts as the
 * other mounts, inside one `LayoutGroup`), the new one animates from the old one's box. The nasiko-ui-lab TokenOps
 * page uses it for the spike clause → day-panel header morph (`SummaryHero`, `DayPanel`: `m.span layoutId`); it
 * needs the layout features, which the app scope loads (`MotionProvider features="max"`, lazy `domMax`). Under
 * reduced motion or a `none` scope the host's `MotionConfig`/`skipAnimations` make the swap instant. Without a
 * `layoutId` it is a plain span.
 */
export function MorphSpan({
  layoutId,
  ...props
}: Omit<ComponentProps<typeof m.span>, "layoutId"> & { layoutId?: string | undefined }) {
  return <m.span {...(layoutId !== undefined ? { layoutId } : {})} {...props} />;
}
