"use client";
/**
 * Lab addition (not in the migration kit): the 2 px top edge every bar carries in light mode
 * (DESIGN.md "Charts"). Dark mode sets the edge token equal to the fill, so it disappears there.
 * With `growFrom`, the edge rises with its bar during the enter animation.
 */
import { type Transition } from "motion/react";
import * as m from "motion/react-m";

const EDGE_PX = 2;

export function BarEdge({
  x,
  y,
  width,
  height,
  edge,
  growFrom,
  transition,
  opacity = 1,
}: {
  x: number;
  y: number;
  width: number;
  height: number;
  edge?: string | undefined;
  /** The baseline y the bar grows from; omit for a static edge. */
  growFrom?: number | undefined;
  transition?: Transition | undefined;
  opacity?: number | undefined;
}) {
  if (!edge || height <= 0 || width <= 0) return null;
  const h = Math.min(EDGE_PX, height);
  if (growFrom !== undefined) {
    return (
      <m.rect
        fill={edge}
        x={x}
        width={width}
        initial={{ y: growFrom, height: 0 }}
        animate={{ y, height: h }}
        {...(transition !== undefined && { transition })}
      />
    );
  }
  return (
    <rect
      fill={edge}
      x={x}
      y={y}
      width={width}
      height={h}
      opacity={opacity}
      style={{ transition: "opacity 0.15s ease-in-out" }}
    />
  );
}
