export function formatLatencyMs(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) {
    return "—";
  }
  if (ms < 1000) {
    return `${Math.round(ms)} ms`;
  }
  return `${(ms / 1000).toFixed(1)} s`;
}

export function formatPercent(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) {
    return "—";
  }
  return `${value.toFixed(1)}%`;
}

export function formatCount(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) {
    return "—";
  }
  return value.toLocaleString();
}

export function formatHourLabel(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

export function formatCostUsd(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) {
    return "—";
  }
  if (value < 0.01) {
    return `$${value.toFixed(4)}`;
  }
  return `$${value.toFixed(2)}`;
}

export function formatShortId(id: string, head = 8, tail = 4): string {
  if (id.length <= head + tail + 1) {
    return id;
  }
  return `${id.slice(0, head)}…${id.slice(-tail)}`;
}

/**
 * Tier B — Display a quality/satisfaction score. Scores live on whatever scale
 * the underlying annotation uses (often 0–1, 0–5, or 0–100). We:
 *   - if value is 0..1, show one decimal as %
 *   - if value is 1..5, show "/5"
 *   - if value is 5..100, show "/100"
 *   - else, show the raw rounded number
 */
export function formatQualityScore(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) {
    return "—";
  }
  if (value > 0 && value <= 1) {
    return `${(value * 100).toFixed(0)}%`;
  }
  if (value <= 5) {
    return `${value.toFixed(1)} / 5`;
  }
  if (value <= 100) {
    return `${value.toFixed(0)} / 100`;
  }
  return value.toFixed(1);
}

/** Tier B — "3.4 steps" or "—" when no orchestration data. */
export function formatSteps(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) {
    return "—";
  }
  if (value >= 10) {
    return `${Math.round(value)} steps`;
  }
  return `${value.toFixed(1)} steps`;
}

export function formatDateTime(iso: string | null | undefined): string {
  if (!iso) {
    return "—";
  }
  const d = new Date(iso);
  if (!Number.isFinite(d.getTime())) {
    return "—";
  }
  return d.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}
