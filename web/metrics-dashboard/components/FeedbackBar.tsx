"use client";

/**
 * Tier B — Stacked label-distribution bar shown inside the "User feedback"
 * KPI card. Renders top label fractions (e.g. "thumbs_up 0.82" /
 * "thumbs_down 0.12" / "neutral 0.06") as a single thin bar with tonal colors
 * inferred from common label keywords. Falls back to neutral for anything we
 * can't classify, so a custom rubric ("excellent" / "poor") still renders.
 */

type LabelEntry = { label: string; fraction: number };

type Props = {
  labels: LabelEntry[] | null | undefined;
  /** Max items to render (rest collapse into "Other"). */
  maxItems?: number;
};

type Tone = "good" | "bad" | "neutral";

function classify(label: string): Tone {
  const lower = label.toLowerCase();
  if (
    /^(thumbs_?up|up|positive|pos|good|excellent|great|approved|like|yes|correct|ok)$/.test(
      lower,
    )
  ) {
    return "good";
  }
  if (
    /^(thumbs_?down|down|negative|neg|bad|poor|fail|failed|error|disliked|no|incorrect)$/.test(
      lower,
    )
  ) {
    return "bad";
  }
  return "neutral";
}

function pretty(label: string): string {
  return label
    .replace(/[_-]+/g, " ")
    .replace(/\b\w/g, (c) => c.toUpperCase());
}

export function FeedbackBar({ labels, maxItems = 4 }: Props) {
  if (!labels || labels.length === 0) {
    return null;
  }

  const sum = labels.reduce((s, l) => s + Math.max(0, l.fraction), 0);
  if (sum <= 0) {
    return null;
  }

  const sorted = [...labels].sort((a, b) => b.fraction - a.fraction);
  const head = sorted.slice(0, maxItems);
  const rest = sorted.slice(maxItems);
  const restFraction = rest.reduce((s, l) => s + Math.max(0, l.fraction), 0);

  const segments: Array<LabelEntry & { tone: Tone | "muted" }> = head.map(
    (l) => ({
      label: l.label,
      fraction: Math.max(0, l.fraction) / sum,
      tone: classify(l.label),
    }),
  );
  if (restFraction > 0) {
    segments.push({
      label: "Other",
      fraction: restFraction / sum,
      tone: "muted",
    });
  }

  return (
    <div className="feedback-bar" role="img" aria-label="User feedback distribution">
      <div className="feedback-bar__track" aria-hidden="true">
        {segments.map((seg, i) => (
          <span
            key={`${seg.label}-${i}`}
            className={`feedback-bar__seg feedback-bar__seg--${seg.tone}`}
            style={{ flex: `${(seg.fraction * 100).toFixed(2)} 0 0` }}
            title={`${pretty(seg.label)} · ${(seg.fraction * 100).toFixed(1)}%`}
          />
        ))}
      </div>
      <ul className="feedback-bar__legend">
        {segments.slice(0, 3).map((seg) => (
          <li key={`legend-${seg.label}`}>
            <span
              className={`legend-swatch feedback-bar__swatch--${seg.tone}`}
              aria-hidden="true"
            />
            <span className="feedback-bar__label">{pretty(seg.label)}</span>
            <span className="feedback-bar__pct">
              {(seg.fraction * 100).toFixed(0)}%
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}
