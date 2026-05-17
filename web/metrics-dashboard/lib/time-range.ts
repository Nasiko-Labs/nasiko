/** Shared time-range helpers for metrics dashboard filters. */

export type MetricsTimeRange = {
  /** ISO UTC — Nasiko `start_time` query */
  startTimeIso: string;
  /** ISO UTC — client-side end bound (inclusive) */
  endTimeIso: string;
  startMs: number;
  endMs: number;
  hours: number;
  /** Label for KPI footer, e.g. `custom` or `24h` */
  window: string;
};

const MAX_RANGE_MS = 7 * 24 * 60 * 60 * 1000;

export function toDatetimeLocalValue(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function parseDatetimeLocal(value: string): Date | null {
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }
  const ms = Date.parse(trimmed);
  if (!Number.isFinite(ms)) {
    return null;
  }
  return new Date(ms);
}

export function defaultTimeRange(now = Date.now()): MetricsTimeRange {
  return buildTimeRange(new Date(now - 24 * 60 * 60 * 1000), new Date(now));
}

export function buildTimeRange(from: Date, to: Date): MetricsTimeRange {
  const startMs = from.getTime();
  const endMs = to.getTime();
  const hours = Math.max(1, Math.ceil((endMs - startMs) / (60 * 60 * 1000)));
  return {
    startTimeIso: from.toISOString(),
    endTimeIso: to.toISOString(),
    startMs,
    endMs,
    hours,
    window: "custom",
  };
}

export function presetTimeRange(
  preset: "1h" | "24h" | "7d",
  now = Date.now(),
): MetricsTimeRange {
  const to = new Date(now);
  const ms =
    preset === "1h"
      ? 60 * 60 * 1000
      : preset === "7d"
        ? 7 * 24 * 60 * 60 * 1000
        : 24 * 60 * 60 * 1000;
  const range = buildTimeRange(new Date(now - ms), to);
  return { ...range, window: preset };
}

export function parseTimeRangeQuery(
  searchParams: URLSearchParams,
  now = Date.now(),
): MetricsTimeRange | { error: string } {
  const startRaw = searchParams.get("startTime")?.trim();
  const endRaw = searchParams.get("endTime")?.trim();

  if (startRaw && endRaw) {
    const from = new Date(startRaw);
    const to = new Date(endRaw);
    if (!Number.isFinite(from.getTime()) || !Number.isFinite(to.getTime())) {
      return { error: "Invalid startTime or endTime" };
    }
    if (to.getTime() <= from.getTime()) {
      return { error: "endTime must be after startTime" };
    }
    if (to.getTime() - from.getTime() > MAX_RANGE_MS) {
      return { error: "Time range cannot exceed 7 days" };
    }
    return buildTimeRange(from, to);
  }

  const window = (searchParams.get("window") ?? "24h").trim().toLowerCase();
  if (window === "1h") {
    return presetTimeRange("1h", now);
  }
  if (window === "7d") {
    return presetTimeRange("7d", now);
  }
  if (window === "24h") {
    return { ...presetTimeRange("24h", now), window: "24h" };
  }

  return { error: `Unsupported window "${window}". Use 1h, 24h, 7d, or startTime/endTime.` };
}

export function timeRangeQueryString(range: MetricsTimeRange): string {
  return `startTime=${encodeURIComponent(range.startTimeIso)}&endTime=${encodeURIComponent(range.endTimeIso)}`;
}

export function formatRangeLabel(range: MetricsTimeRange): string {
  const fmt = (ms: number) =>
    new Date(ms).toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  return `${fmt(range.startMs)} – ${fmt(range.endMs)}`;
}
