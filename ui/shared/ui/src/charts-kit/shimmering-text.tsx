// Destination: `useReducedMotion()` → `useKitReducedMotion()` (./use-kit-reduced-motion.ts), so the kit's at-rest
// path also follows the motion host's clamped mode (surface `none`/`reduced`, ADR-0006); the OS rule is unchanged.
"use client";

import { type Variants } from "motion/react";
import * as m from "motion/react-m";
import { type ComponentProps, useCallback } from "react";
import { cn } from "../lib/cn";
import { useKitReducedMotion } from "./use-kit-reduced-motion";

export type ShimmeringTextProps = Omit<ComponentProps<typeof m.span>, "children"> & {
  /** The text to render with the shimmering effect. */
  text: string;
  /**
   * Duration in seconds for one shimmer cycle.
   * @defaultValue 1
   */
  duration?: number | undefined;
  /**
   * Pause the shimmer (e.g. when the hero leaves the viewport).
   * @defaultValue false
   */
  paused?: boolean | undefined;
  /**
   * Legacy alias for `paused`.
   * @defaultValue false
   */
  isStopped?: boolean | undefined;
};

export function ShimmeringText({
  text,
  duration = 1,
  isStopped = false,
  paused = false,
  className,
  ...props
}: ShimmeringTextProps) {
  const reducedMotion = useKitReducedMotion();
  const stopped = isStopped || paused || reducedMotion;

  const createCharVariants = useCallback(
    (charIndex: number): Variants => ({
      running: {
        color: ["var(--color)", "var(--shimmering-color)", "var(--color)"],
        transition: {
          duration,
          repeat: Number.POSITIVE_INFINITY,
          repeatType: "loop",
          repeatDelay: text.length * 0.05,
          delay: (charIndex * duration) / text.length,
          ease: "easeInOut",
        },
      },
      stopped: {
        color: "var(--color)",
        transition: {
          duration: duration * 0.5,
          ease: "easeOut",
        },
      },
    }),
    [duration, text.length],
  );

  return (
    <m.span
      className={cn(
        "inline-flex items-center leading-none select-none",
        "[--color:var(--muted-foreground)] [--shimmering-color:var(--foreground)]",
        className,
      )}
      {...props}
    >
      {text.split("").map((char, index) => (
        <m.span
          animate={stopped ? "stopped" : "running"}
          aria-hidden
          className="inline-block leading-none whitespace-pre"
          initial="stopped"
          // one span per character of a static label; position is its identity (the lab disables @eslint-react/no-array-index-key here)
          key={index}
          variants={createCharVariants(index)}
        >
          {char}
        </m.span>
      ))}
      <span className="sr-only">{text}</span>
    </m.span>
  );
}
