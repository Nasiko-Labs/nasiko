/**
 * Delta polarity per metric (plan A7). Colour is never the only signal: every delta
 * also carries an arrow and a sign.
 */
export type Polarity = 'down-good' | 'up-good' | 'neutral'

export const POLARITY = {
  spend: 'down-good',
  costPerOp: 'down-good',
  p95: 'down-good',
  p99: 'down-good',
  latency: 'down-good',
  tokens: 'down-good',
  cacheRatio: 'up-good',
  operations: 'neutral',
  toolCalls: 'neutral',
  agents: 'neutral',
} as const satisfies Record<string, Polarity>

export type Tone = 'good' | 'bad' | 'neutral' | 'none'

/** Text colour per tone (Delta, and deltas quoted inside a sentence). */
export const TONE_CLASS: Record<Tone, string> = {
  good: 'text-success',
  bad: 'text-destructive',
  neutral: 'text-muted-foreground',
  none: 'text-muted-foreground',
}

export interface DeltaView {
  tone: Tone
  arrow: '▲' | '▼' | '–' | ''
  text: string
  /** Screen-reader phrase, e.g. "up 12.3% versus previous period, worse". */
  label: string
}

/**
 * `changePct` is null when the previous value was 0 (server semantics). That is "new"
 * only when the current value is non-zero; 0 → 0 is simply unchanged.
 */
export function describeDelta(
  changePct: number | null | undefined,
  polarity: Polarity,
  current?: number,
): DeltaView {
  if (changePct === null || changePct === undefined || Number.isNaN(changePct)) {
    if (current === 0)
      return {
        tone: 'neutral',
        arrow: '–',
        text: '0.0%',
        label: 'unchanged versus previous period (both zero)',
      }
    return { tone: 'none', arrow: '', text: 'new', label: 'no previous value to compare' }
  }
  if (Math.abs(changePct) < 0.05)
    return { tone: 'neutral', arrow: '–', text: '0.0%', label: 'unchanged versus previous period' }
  const up = changePct > 0
  const tone: Tone =
    polarity === 'neutral' ? 'neutral' : (polarity === 'down-good') === up ? 'bad' : 'good'
  const text = `${up ? '+' : '−'}${Math.abs(changePct).toFixed(1)}%`
  const verdict = tone === 'good' ? ', better' : tone === 'bad' ? ', worse' : ''
  return {
    tone,
    arrow: up ? '▲' : '▼',
    text,
    label: `${up ? 'up' : 'down'} ${Math.abs(changePct).toFixed(1)}% versus previous period${verdict}`,
  }
}
