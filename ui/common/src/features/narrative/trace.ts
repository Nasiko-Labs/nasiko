/**
 * Plain-English narrative for one trace. Pure and deterministic: every sentence is built
 * from span fields, and each clause lists the spans it talks about (hover/focus
 * highlights them).
 *
 * Rules (plan: Trace narrative + design P6/J3/J5):
 * - At most 3 sentences, in the order cause > money > status; the rest go to `details`.
 * - Money comes only from `TraceDetail.cost_summary`; shares are TOKEN shares from
 *   SpanNode ("% of the tokens"), because SpanNode has no cost.
 * - Retry waste in dollars needs SpanDetail costs (fetched by the caller, ≤ 10). With more
 *   retry spans, or a failed fetch, the waste is worded in tokens instead.
 * - A retry loop with no gap between attempts adds a closing "Retry policy" takeaway.
 */
import {
  CLASS_LABEL,
  findRetryLoops,
  isError,
  ownTokens,
  tokenSplit,
  traceDurationMs,
  unrecoveredErrors,
  type FlatSpan,
  type RetryLoop,
} from '@/features/observability/spans'
import { NO_BACKOFF_MS, RETRY_FETCH_CAP, TIMEOUT_MIN_MS } from '@/features/observability/tuning'
import { fmtLatency, fmtMoney } from '@/lib/format'

export interface Clause {
  text: string
  /** Span ids (base64 `id`) this clause is about. */
  spanIds: string[]
}

export interface TraceNarrative {
  /** ≤ 3 sentences, each made of clauses. */
  sentences: Clause[][]
  details: Clause[]
  /** Closing resolution line, e.g. "Retry policy: 6 attempts, no backoff." */
  takeaway: Clause | null
  healthy: boolean
}

export interface TraceNarrativeInput {
  totalCost: number
  spans: FlatSpan[]
  /** Display name for an agent-call target, when known (a2a spans). */
  agentName?: (span: FlatSpan) => string | undefined
  /**
   * SpanDetail costs for the retry-waste spans, keyed by span id. `undefined` = not
   * fetched (or a fetch failed): the waste is worded in tokens.
   */
  wasteCosts?: Map<string, number>
}

const code = (name: string) => `\`${name.replace(/^tool\./, '')}\``
const pct = (part: number, total: number) => (total > 0 ? Math.round((part / total) * 100) : 0)

/** The spans whose cost counts as retry waste: the attempts plus their re-plans. */
function wasteSpans(loop: RetryLoop): FlatSpan[] {
  return [...loop.attempts, ...loop.between]
}

/** The waste spans that can carry cost (tokens > 0): the only ones worth a SpanDetail fetch. */
export function wasteCostSpans(loop: RetryLoop): FlatSpan[] {
  return wasteSpans(loop).filter((s) => ownTokens(s.node) > 0)
}

/** Should the caller fetch SpanDetail costs for this loop? (≤ RETRY_FETCH_CAP spans.) */
export function wasteFetchable(loop: RetryLoop): boolean {
  return wasteCostSpans(loop).length <= RETRY_FETCH_CAP
}

function cascade(spans: FlatSpan[], agentName: TraceNarrativeInput['agentName']): Clause | null {
  const calls = spans.filter((s) => s.cls === 'agent')
  if (!calls.length) return null
  const names = new Set(calls.map((c) => agentName?.(c) ?? 'another agent'))
  const who = names.size === 1 ? [...names][0] : `${names.size} agents`
  return {
    text: `called ${who} ${calls.length} time${calls.length === 1 ? '' : 's'} through the proxy`,
    spanIds: calls.map((c) => c.node.id),
  }
}

export function traceNarrative({
  totalCost,
  spans,
  agentName,
  wasteCosts,
}: TraceNarrativeInput): TraceNarrative {
  const [loop] = findRetryLoops(spans)
  const errors = spans.filter((s) => isError(s.node))
  const split = tokenSplit(spans)
  const duration = traceDurationMs(spans)
  const calls = cascade(spans, agentName)
  const details: Clause[] = []
  const sentences: Clause[][] = []
  const money: Clause = { text: `This trace cost ${fmtMoney(totalCost)}`, spanIds: [] }

  // Token share by kind, always available as a detail (the biggest non-zero class).
  const topClass = (Object.entries(split.byClass) as [keyof typeof CLASS_LABEL, number][]).sort(
    (a, b) => b[1] - a[1],
  )[0]
  const shareClause: Clause | null =
    split.total > 0 && topClass[1] > 0
      ? {
          text: `${CLASS_LABEL[topClass[0]]} calls used ${pct(topClass[1], split.total)}% of the tokens`,
          spanIds: spans
            .filter((s) => s.cls === topClass[0] && ownTokens(s.node) > 0)
            .map((s) => s.node.id),
        }
      : null

  if (!loop && !errors.length) {
    const text = `This trace cost ${fmtMoney(totalCost)} and finished in ${fmtLatency(duration)} with no errors`
    sentences.push([{ text, spanIds: [] }])
    if (calls) sentences.push([{ text: `It ${calls.text}`, spanIds: calls.spanIds }])
    if (shareClause) details.push(shareClause)
    return { sentences: sentences.slice(0, 3), details, takeaway: null, healthy: true }
  }

  // 1. Cause.
  if (loop) {
    // N attempts are N − 1 retries.
    const retries = loop.attempts.length - 1
    const cause: Clause[] = [
      {
        text: `The planner retried ${code(loop.name)} ${retries} time${retries === 1 ? '' : 's'} after ${loop.failures === loop.attempts.length ? 'errors' : `${loop.failures} error${loop.failures === 1 ? '' : 's'}`}`,
        spanIds: loop.attempts.map((a) => a.node.id),
      },
    ]
    if (calls) cause.push({ text: ` and ${calls.text}`, spanIds: calls.spanIds })
    sentences.push(cause)
  } else {
    const last = errors[errors.length - 1]
    sentences.push([{ text: `${code(last.node.name)} returned an error`, spanIds: [last.node.id] }])
    if (calls) details.push({ text: `It ${calls.text}`, spanIds: calls.spanIds })
  }

  // 2. Money, with retry waste.
  const moneySentence: Clause[] = [money]
  if (loop) {
    const waste = wasteSpans(loop)
    const ids = waste.map((s) => s.node.id)
    // Zero-token spans cost nothing; every token-bearing one needs its SpanDetail cost.
    const priced = wasteCostSpans(loop)
    const dollars =
      wasteCosts && wasteFetchable(loop) && priced.every((s) => wasteCosts.has(s.node.id))
        ? priced.reduce((acc, s) => acc + (wasteCosts.get(s.node.id) ?? 0), 0)
        : null
    if (dollars !== null && dollars > 0) {
      moneySentence.push({ text: `; retries cost ${fmtMoney(dollars)}`, spanIds: ids })
    } else {
      const tokens = waste.reduce((acc, s) => acc + ownTokens(s.node), 0)
      if (tokens > 0)
        moneySentence.push({
          text: `; retries drove ${pct(tokens, split.total)}% of the tokens`,
          spanIds: ids,
        })
    }
  }
  sentences.push(moneySentence)
  if (shareClause) details.push(shareClause)

  // 3. Status: only an error nothing recovered from makes the trace fail.
  const lastError = unrecoveredErrors(spans).reduce<FlatSpan | undefined>(
    (a, b) => (!a || b.startMs >= a.startMs ? b : a),
    undefined,
  )
  if (lastError) {
    const timedOut = lastError.durationMs >= TIMEOUT_MIN_MS
    sentences.push([
      {
        text: timedOut
          ? `It failed when the last attempt timed out at ${fmtLatency(lastError.durationMs)}`
          : `It ended with an error in ${code(lastError.node.name)}`,
        spanIds: [lastError.node.id],
      },
    ])
  } else {
    sentences.push([{ text: `It recovered and finished in ${fmtLatency(duration)}`, spanIds: [] }])
  }

  const takeaway =
    loop && loop.maxGapMs < NO_BACKOFF_MS
      ? {
          text: `Retry policy: ${loop.attempts.length} attempts, no backoff.`,
          spanIds: loop.attempts.map((a) => a.node.id),
        }
      : null
  return {
    sentences: sentences.slice(0, 3),
    details: [...sentences.slice(3).flat(), ...details],
    takeaway,
    healthy: false,
  }
}

/** Sentences as plain strings (tests, aria text, copy). */
export function narrativeText(n: TraceNarrative): string[] {
  return n.sentences.map((s) => `${s.map((c) => c.text).join('')}.`)
}
