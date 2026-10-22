// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/components/Sparkline.tsx. Local edits: Prettier formatting.
/**
 * A row's spend over the window as one line (design "Home": entity colour, never a health colour). Decorative: the
 * figure beside it is the value, so the line is hidden from assistive tech.
 */
import { SPARK_H as H, SPARK_W as W } from "../tuning";

export function Sparkline({ values }: { values: readonly number[] }) {
  if (values.length < 2) return null;
  const max = Math.max(...values);
  const x = (i: number) => (i / (values.length - 1)) * W;
  // A flat zero line sits on the baseline; the 1px inset keeps the stroke inside the box.
  const y = (v: number) => (max > 0 ? H - 1 - (v / max) * (H - 2) : H - 1);
  const d = values.map((v, i) => `${i ? "L" : "M"}${x(i).toFixed(1)},${y(v).toFixed(1)}`).join("");
  return (
    <svg
      width={W}
      height={H}
      viewBox={`0 0 ${W} ${H}`}
      aria-hidden
      data-testid="home-sparkline"
      className="shrink-0 overflow-visible"
    >
      <path
        d={d}
        fill="none"
        stroke="var(--chart-line-primary)"
        strokeWidth={1.5}
        strokeLinejoin="round"
        strokeLinecap="round"
      />
    </svg>
  );
}
