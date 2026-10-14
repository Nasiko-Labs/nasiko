// Destination: `useReducedMotion()` → `useKitReducedMotion()` (./use-kit-reduced-motion.ts), so the kit's at-rest
// path also follows the motion host's clamped mode (surface `none`/`reduced`, ADR-0006); the OS rule is unchanged.
"use client";

import type { Transition } from "motion/react";
import * as m from "motion/react-m";
import { clipRevealTransition } from "./animation";
import { useKitReducedMotion } from "./use-kit-reduced-motion";

export type ChartRevealClipMode = "reveal" | "conceal";

export interface ChartRevealClipProps {
  clipPathId: string;
  height: number;
  targetWidth: number;
  enterTransition?: Transition | undefined;
  /** Bumps when motion settings change to replay the reveal. */
  revealEpoch: number;
  /** Extra inset around the clip rect so edge glyphs are not cut off. */
  padding?: number | undefined;
  /** When false, clip stays at full width (no grow animation). */
  animating?: boolean | undefined;
  /** Reveal grows 0 → full; conceal shrinks full → 0 (ready → loading). */
  mode?: ChartRevealClipMode | undefined;
  /** Called when a conceal animation finishes. */
  onComplete?: (() => void) | undefined;
}

/**
 * Left-to-right clip reveal for cartesian series.
 * Grows clip rect width from 0 → full (true LTR; scaleX is avoided — it reveals from center).
 */
export function ChartRevealClip({
  clipPathId,
  height,
  targetWidth,
  enterTransition,
  revealEpoch,
  padding = 0,
  animating = true,
  mode = "reveal",
  onComplete,
}: ChartRevealClipProps) {
  const transition = clipRevealTransition(enterTransition);
  // Lab: reduced motion shows the series at once (no left-to-right sweep).
  const reducedMotion = useKitReducedMotion();
  const paddedWidth = Math.max(0, targetWidth + padding * 2);
  const paddedHeight = height + padding * 2;

  if (!animating || reducedMotion) {
    return (
      <clipPath id={clipPathId}>
        <rect height={paddedHeight} width={paddedWidth} x={-padding} y={-padding} />
      </clipPath>
    );
  }

  if (mode === "conceal") {
    // Mirror the LTR reveal: advance the clip's left edge rightward while width
    // shrinks (same geometry as `LineLoadingPulseStroke` exit half-cycle).
    const rightEdge = -padding + paddedWidth;

    return (
      <clipPath id={clipPathId}>
        <m.rect
          animate={{ width: 0, x: rightEdge }}
          height={paddedHeight}
          initial={{ width: paddedWidth, x: -padding }}
          key={`conceal-${revealEpoch}`}
          onAnimationComplete={() => onComplete?.()}
          transition={transition}
          y={-padding}
        />
      </clipPath>
    );
  }

  return (
    <clipPath id={clipPathId}>
      <m.rect
        animate={{ width: paddedWidth }}
        height={paddedHeight}
        initial={{ width: 0 }}
        key={`reveal-${revealEpoch}`}
        transition={transition}
        width={paddedWidth}
        x={-padding}
        y={-padding}
      />
    </clipPath>
  );
}
