"use client";

import type { Variants } from "motion/react";
import * as m from "motion/react-m";
import { memo } from "react";
import { DEFAULT_CHART_ENTER_EASE } from "./animation";

export interface SeriesPointMarkerStyle {
  /** Fill color for the inner circle */
  fill?: string | undefined;
  /** Outer ring stroke color. Default: same as `fill` */
  stroke?: string | undefined;
  /** Outer ring stroke width in px. Default: 2. Set to 0 to disable. */
  strokeWidth?: number | undefined;
  /** Gap between the inner fill and outer ring in px. Default: 2 */
  ringGap?: number | undefined;
  /** Optional outer outline beyond the ring. Default: 0 */
  outlineWidth?: number | undefined;
  /** Outer outline color. Default: same as `stroke` */
  outlineColor?: string | undefined;
  /** Point radius in px. Default: 5 */
  radius?: number | undefined;
  /** Dim non-active points while hovering. Default: true */
  fadeOnHover?: boolean | undefined;
  /** Opacity for non-hovered points when `fadeOnHover` is true. Default: 0.5 */
  inactiveOpacity?: number | undefined;
  /**
   * Blur in px for non-hovered points when `fadeOnHover` is true.
   * Applied once on the dimmed layer (not per dot) for performance. Default: 2
   */
  inactiveBlur?: number | undefined;
  /** Initial blur in px during enter animation. Default: 2 */
  enterBlur?: number | undefined;
  /** Enlarge the active point while hovering. Default: true */
  showActiveHighlight?: boolean | undefined;
}

interface MarkerCirclesProps {
  fill?: string | undefined;
  stroke?: string | undefined;
  strokeWidth: number;
  ringGap: number;
  outlineWidth: number;
  outlineColor?: string | undefined;
  radius: number;
}

function MarkerCircles({
  fill,
  stroke,
  strokeWidth,
  ringGap,
  outlineWidth,
  outlineColor,
  radius,
}: MarkerCirclesProps) {
  const resolvedStroke = stroke ?? fill ?? "currentColor";
  const resolvedOutlineColor = outlineColor ?? resolvedStroke;
  const ringOuter = strokeWidth > 0 ? radius + ringGap + strokeWidth : radius;
  const outlineRadius = outlineWidth > 0 ? ringOuter + outlineWidth / 2 : 0;

  return (
    <>
      {outlineWidth > 0 ? (
        <circle
          cx={0}
          cy={0}
          fill="none"
          r={outlineRadius}
          stroke={resolvedOutlineColor}
          strokeWidth={outlineWidth}
        />
      ) : null}
      <circle cx={0} cy={0} fill={fill} r={radius} />
      {strokeWidth > 0 ? (
        <circle
          cx={0}
          cy={0}
          fill="none"
          r={radius + ringGap + strokeWidth / 2}
          stroke={resolvedStroke}
          strokeWidth={strokeWidth}
        />
      ) : null}
    </>
  );
}

export interface StaticSeriesPointMarkerProps extends SeriesPointMarkerStyle {
  cx: number;
  cy: number;
  scale?: number | undefined;
}

export const StaticSeriesPointMarker = memo(function StaticSeriesPointMarker({
  cx,
  cy,
  scale = 1,
  fill,
  stroke,
  strokeWidth = 2,
  ringGap = 2,
  outlineWidth = 0,
  outlineColor,
  radius = 5,
}: StaticSeriesPointMarkerProps) {
  return (
    <g transform={`translate(${cx}, ${cy}) scale(${scale})`}>
      <MarkerCircles
        fill={fill}
        outlineColor={outlineColor}
        outlineWidth={outlineWidth}
        radius={radius}
        ringGap={ringGap}
        stroke={stroke}
        strokeWidth={strokeWidth}
      />
    </g>
  );
});

export interface SeriesPointMarkerProps extends SeriesPointMarkerStyle {
  dataKey: string;
  index: number;
  cx: number;
  cy: number;
  revealDelay: number;
  revealEpoch: number;
  enterDuration: number;
}

/** Motion enter marker — used only while the chart reveal is running. */
export function SeriesPointMarker({
  dataKey,
  index,
  cx,
  cy,
  enterBlur = 2,
  revealDelay,
  revealEpoch,
  enterDuration,
  fill,
  stroke,
  strokeWidth = 2,
  ringGap = 2,
  outlineWidth = 0,
  outlineColor,
  radius = 5,
}: SeriesPointMarkerProps) {
  const variants: Variants = {
    hidden: {
      opacity: 0,
      filter: `blur(${enterBlur}px)`,
      scale: 1,
    },
    visible: {
      opacity: 1,
      filter: "blur(0px)",
      scale: 1,
      transition: {
        delay: revealDelay,
        duration: enterDuration,
        ease: DEFAULT_CHART_ENTER_EASE,
      },
    },
  };

  return (
    <g transform={`translate(${cx}, ${cy})`}>
      <m.g
        animate="visible"
        initial="hidden"
        key={`${dataKey}-${index}-${revealEpoch}`}
        variants={variants}
      >
        <MarkerCircles
          fill={fill}
          outlineColor={outlineColor}
          outlineWidth={outlineWidth}
          radius={radius}
          ringGap={ringGap}
          stroke={stroke}
          strokeWidth={strokeWidth}
        />
      </m.g>
    </g>
  );
}
