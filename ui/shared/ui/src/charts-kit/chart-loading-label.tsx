"use client";

import * as m from "motion/react-m";
import { cn } from "../lib/cn";
import { ShimmeringText } from "./shimmering-text";
import {
  LINE_LOADING_PULSE_EASE,
  LOADING_LABEL_EXIT_S,
  LOADING_LABEL_EXIT_Y_PX,
} from "./line-loading-timing";

export interface ChartLoadingLabelProps {
  /** Label shown centered over the chart. */
  text?: string | undefined;
  className?: string | undefined;
  /** Animate down, fade, and blur during loading → ready handoff. */
  exiting?: boolean | undefined;
}

export function ChartLoadingLabel({
  text = "Loading",
  className,
  exiting = false,
}: ChartLoadingLabelProps) {
  if (!text.trim()) {
    return null;
  }

  return (
    <m.div
      animate={{
        y: exiting ? LOADING_LABEL_EXIT_Y_PX : 0,
        opacity: exiting ? 0 : 1,
        filter: exiting ? "blur(2px)" : "blur(0px)",
      }}
      aria-live="polite"
      className={cn(
        "pointer-events-none absolute inset-0 flex items-center justify-center",
        className,
      )}
      initial={false}
      role="status"
      transition={{
        duration: LOADING_LABEL_EXIT_S,
        ease: [...LINE_LOADING_PULSE_EASE],
      }}
    >
      <ShimmeringText
        className="text-sm font-medium tracking-wide [--color:var(--muted-foreground)] [--shimmering-color:var(--foreground)]"
        text={text}
      />
    </m.div>
  );
}

export default ChartLoadingLabel;
