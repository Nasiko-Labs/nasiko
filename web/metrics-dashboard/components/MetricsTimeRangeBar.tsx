"use client";

import { useEffect, useState } from "react";
import {
  buildTimeRange,
  formatRangeLabel,
  parseDatetimeLocal,
  presetTimeRange,
  toDatetimeLocalValue,
  type MetricsTimeRange,
} from "@/lib/time-range";

export type TimeRangeDraft = {
  fromLocal: string;
  toLocal: string;
};

type Props = {
  range: MetricsTimeRange;
  onApply: (range: MetricsTimeRange) => void;
  disabled?: boolean;
  compact?: boolean;
  idPrefix?: string;
};

export function draftFromRange(range: MetricsTimeRange): TimeRangeDraft {
  return {
    fromLocal: toDatetimeLocalValue(new Date(range.startMs)),
    toLocal: toDatetimeLocalValue(new Date(range.endMs)),
  };
}

export function rangeFromDraft(draft: TimeRangeDraft): MetricsTimeRange | null {
  const from = parseDatetimeLocal(draft.fromLocal);
  const to = parseDatetimeLocal(draft.toLocal);
  if (!from || !to) {
    return null;
  }
  if (to.getTime() <= from.getTime()) {
    return null;
  }
  return buildTimeRange(from, to);
}

export function MetricsTimeRangeBar({
  range,
  onApply,
  disabled,
  compact,
  idPrefix = "metrics",
}: Props) {
  const [draft, setDraft] = useState(() => draftFromRange(range));
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setDraft(draftFromRange(range));
  }, [range.startMs, range.endMs]);

  const applyDraft = () => {
    const next = rangeFromDraft(draft);
    if (!next) {
      setError("End time must be after start time.");
      return;
    }
    setError(null);
    onApply(next);
  };

  const applyPreset = (preset: "1h" | "24h" | "7d") => {
    const next = presetTimeRange(preset);
    setDraft(draftFromRange(next));
    setError(null);
    onApply(next);
  };

  return (
    <div
      className={`time-filter${compact ? " time-filter--compact" : ""}`}
      role="search"
      aria-label="Filter by date and time"
    >
      {!compact && <p className="time-filter__label">Time range</p>}
      <div className="time-filter__row">
        <div className="field field--inline">
          <label htmlFor={`${idPrefix}-preset`}>Preset</label>
          <select
            id={`${idPrefix}-preset`}
            disabled={disabled}
            defaultValue=""
            onChange={(e) => {
              const v = e.target.value;
              if (v === "1h" || v === "24h" || v === "7d") {
                applyPreset(v);
              }
              e.target.value = "";
            }}
          >
            <option value="" disabled>
              Quick range…
            </option>
            <option value="1h">Last 1 hour</option>
            <option value="24h">Last 24 hours</option>
            <option value="7d">Last 7 days</option>
          </select>
        </div>
        <div className="field field--inline">
          <label htmlFor={`${idPrefix}-from`}>From</label>
          <input
            id={`${idPrefix}-from`}
            type="datetime-local"
            disabled={disabled}
            value={draft.fromLocal}
            onChange={(e) => setDraft((d) => ({ ...d, fromLocal: e.target.value }))}
          />
        </div>
        <div className="field field--inline">
          <label htmlFor={`${idPrefix}-to`}>To</label>
          <input
            id={`${idPrefix}-to`}
            type="datetime-local"
            disabled={disabled}
            value={draft.toLocal}
            onChange={(e) => setDraft((d) => ({ ...d, toLocal: e.target.value }))}
          />
        </div>
        <button
          type="button"
          className="btn btn--primary btn--sm"
          disabled={disabled}
          onClick={applyDraft}
        >
          Apply
        </button>
      </div>
      {error && (
        <p className="time-filter__error" role="alert">
          {error}
        </p>
      )}
      <p className="time-filter__hint">
        Active range: {formatRangeLabel(range)}
        {compact ? " (narrows table only)" : ""}
      </p>
    </div>
  );
}
