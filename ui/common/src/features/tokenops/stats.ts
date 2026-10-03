/** Small statistics shared by the forecast (F1) and the cost × p95 quadrants (F5). */
export function median(values: number[]): number {
  const s = [...values].sort((a, b) => a - b)
  const mid = Math.floor(s.length / 2)
  return s.length % 2 ? s[mid] : (s[mid - 1] + s[mid]) / 2
}

/** Log-axis ticks at 1× and 3× each decade inside the domain, thinned to about one per 90 px. */
export function logTicks([lo, hi]: [number, number], width: number): number[] {
  const all: number[] = []
  for (let e = Math.floor(Math.log10(lo)); e <= Math.ceil(Math.log10(hi)); e++) {
    for (const m of [1, 3]) {
      const v = m * 10 ** e
      if (v >= lo && v <= hi) all.push(v)
    }
  }
  const max = Math.max(2, Math.floor(width / 90))
  const step = Math.ceil(all.length / max)
  return all.filter((_, i) => i % step === 0)
}
