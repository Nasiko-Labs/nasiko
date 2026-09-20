// Compact number/duration formatting shared by the workflow pages.

/** "980 ms" / "2.3 s" / "4 min" / "1 h 12 min" from a millisecond count. */
export function fmtDuration(ms) {
  if (ms == null || Number.isNaN(ms)) return '—';
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  const mins = Math.floor(ms / 60_000);
  if (mins < 60) return `${mins} min`;
  return `${Math.floor(mins / 60)} h ${mins % 60} min`;
}

/** "365 tokens" / "12.4k tokens"; empty string when zero/absent. */
export function fmtTokens(n) {
  if (!n) return '';
  const count = n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n);
  return `${count} tokens`;
}
