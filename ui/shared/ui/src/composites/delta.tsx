// Ported from nasiko-ui-lab@1b99bdda common/src/components/shared/delta.tsx. Local edits: imports mapped to
// ../lib/cn and ../lib/delta; optional props typed `| undefined` (exactOptionalPropertyTypes).
import { cn } from "../lib/cn";
import { describeDelta, TONE_CLASS, type Polarity } from "../lib/delta";

/** Arrow + sign + colour (TokenOps A7); never colour alone. `current` lets 0 → 0 read as unchanged, not "new". */
export function Delta({
  changePct,
  polarity,
  unavailable,
  current,
}: {
  changePct: number | null | undefined;
  polarity: Polarity;
  unavailable?: boolean | undefined;
  current?: number | undefined;
}) {
  if (unavailable) {
    return (
      <span
        className="text-xs text-muted-foreground"
        title="comparison unavailable"
        aria-label="comparison unavailable"
      >
        —
      </span>
    );
  }
  const d = describeDelta(changePct, polarity, current);
  return (
    <span
      className={cn(
        "inline-flex items-center gap-0.5 text-xs font-medium tabular-nums",
        TONE_CLASS[d.tone],
      )}
      aria-label={d.label}
      title={d.label}
    >
      {d.arrow ? <span aria-hidden>{d.arrow}</span> : null}
      {d.text}
    </span>
  );
}
