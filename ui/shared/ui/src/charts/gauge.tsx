"use client";

import { useId } from "react";
import { cn } from "../lib/cn";

/**
 * Gauge: a bounded value against a range, as a single SVG arc (ADR-0003
 * bespoke). The value is text first and picture second: the number renders
 * immediately and never counts up; the arc is a `<title>/<desc>`-labelled
 * `role="meter"` so assistive tech reads value, min and max. The only motion
 * is the arc's stroke-dashoffset transition, driven by `--motion-section`,
 * which the policy zeroes under reduced/none.
 */
export interface GaugeProps {
  readonly value: number | null;
  readonly min?: number;
  readonly max: number;
  readonly label: string;
  /** Formatted value text (caller decides units; `null` renders as "unknown"). */
  readonly formatValue?: (v: number) => string;
  /** Thresholds as fractions of the range, mapped to success/warning/destructive. */
  readonly thresholds?: readonly [warnAt: number, dangerAt: number];
  readonly size?: number;
  readonly className?: string;
}

export function Gauge({
  value,
  min = 0,
  max,
  label,
  formatValue = (v) => String(v),
  thresholds,
  size = 160,
  className,
}: GaugeProps) {
  const id = useId();
  const span = Math.max(max - min, Number.EPSILON);
  const fraction = value === null ? 0 : Math.min(1, Math.max(0, (value - min) / span));
  const r = 54;
  const c = Math.PI * r; // half circle
  const tone =
    value === null
      ? "muted"
      : thresholds && fraction >= thresholds[1]
        ? "destructive"
        : thresholds && fraction >= thresholds[0]
          ? "warning"
          : "success";
  const stroke = {
    muted: "var(--muted-foreground)",
    success: "var(--success)",
    warning: "var(--warning)",
    destructive: "var(--destructive)",
  }[tone];
  const text = value === null ? "unknown" : formatValue(value);
  return (
    <figure
      className={cn("inline-flex flex-col items-center gap-1", className)}
      style={{ width: size }}
    >
      <svg
        viewBox="0 0 128 72"
        width={size}
        height={size * 0.5625}
        role={value === null ? "img" : "meter"}
        aria-labelledby={`${id}-t`}
        aria-describedby={`${id}-d`}
        {...(value === null
          ? {}
          : {
              "aria-valuemin": min,
              "aria-valuemax": max,
              "aria-valuenow": value,
              "aria-valuetext": text,
            })}
      >
        <title id={`${id}-t`}>{label}</title>
        <desc id={`${id}-d`}>
          {value === null ? `${label}: value unknown` : `${label}: ${text} of ${formatValue(max)}`}
        </desc>
        <path
          d="M 10 64 A 54 54 0 0 1 118 64"
          fill="none"
          stroke="var(--chart-grid)"
          strokeWidth={10}
          strokeLinecap="round"
        />
        <path
          d="M 10 64 A 54 54 0 0 1 118 64"
          fill="none"
          stroke={stroke}
          strokeWidth={10}
          strokeLinecap="round"
          strokeDasharray={c}
          strokeDashoffset={c * (1 - fraction)}
          style={{ transition: "stroke-dashoffset var(--motion-section) var(--motion-ease-out)" }}
          data-tone={tone}
        />
        <text
          x="64"
          y="60"
          textAnchor="middle"
          fontSize="18"
          fontWeight={600}
          fill="var(--foreground)"
          aria-hidden="true"
        >
          {text}
        </text>
      </svg>
      <figcaption className="text-xs text-muted-foreground">{label}</figcaption>
    </figure>
  );
}
