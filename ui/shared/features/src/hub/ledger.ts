/**
 * A harness session's ledger (plans/feat-agent-centric.md PR1b; design "Ledger"; eng "Ledger"): one row per turn, in
 * plain language, with what was used and what controlled it. Pure.
 *
 * - Turns come from the chat history (`/api/chat/sessions/{id}/messages`, newest 100 first, each page in the server's
 *   own order, which keeps microsecond timestamps the browser can't parse). Pages are joined oldest first, deduped by
 *   message id, never re-sorted. A message's `external_turn_id` names its turn; without one, a user message starts a
 *   turn and the assistant messages after it join it (a prompt without an id takes its reply's). While older pages are
 *   unloaded the oldest loaded turn may be cut (`partial`) and every total is a loaded subtotal (`complete: false`).
 * - Tool calls are the recorded chat's own (`chat/recorded.ts`); a call whose association is `unknown` is listed under
 *   "Turn not recorded", never guessed onto a turn.
 * - Tokens in count fresh input plus cache reads and cache writes (the server reports them apart). A total counts the
 *   turns that reported the value, so a page never presents a sum over some turns as the session's.
 * - Context, memory and applied controls are proposed per-turn records (AH-7, with CX-V1) on the assistant message's
 *   `metadata.coding_agent`. They're parsed only in preview mode (`preview`), so an unexpected live field never draws.
 *   A control is shown only from a recorded decision (time, rule identity, outcome); current settings are linked
 *   elsewhere, never presented as applied.
 * - Budgets (AH-8, preview): the tightest of the session's, agent's and owner's budgets, by percent used.
 */
import type { RecordedStep } from '@nasiko/features/chat/recorded'
import { codingAgentOf, isObject, toolCallsToSteps } from '@nasiko/features/chat/recorded'
import type { ChatMessage, MessagesPage } from '@nasiko/features/chat/types'
import { contextReport, type ContextReport } from '@nasiko/features/optimization/logic'
import { toCsv, type CsvColumn } from '@nasiko/features/tokenops/csv'
import type { SessionBudget } from './types'

const CONTROL_KINDS = ['tool_rule', 'context', 'budget'] as const
const CONTROL_OUTCOMES = ['blocked', 'asked', 'trimmed', 'warned', 'stopped'] as const
type ControlKind = (typeof CONTROL_KINDS)[number]
type ControlOutcome = (typeof CONTROL_OUTCOMES)[number]

/** A recorded decision a control made on this turn (proposed AH-7). */
export interface AppliedControl {
  /** Unique on the page: the message id and the record's position. */
  key: string
  at: string
  kind: ControlKind
  rule: { id: string; name: string }
  outcome: ControlOutcome
  /** The tool a tool rule acted on. */
  tool?: string
  /** Tokens a context trim removed. */
  tokensSaved?: number
}

interface MemoryUse {
  tokens: number
  items: number | null
}

export interface LedgerTurn {
  key: string
  /** 1-based among the loaded turns, oldest first. */
  n: number
  at: string
  prompt: string | null
  tools: RecordedStep[]
  /** Fresh input plus cache reads and cache writes. */
  tokensIn: number | null
  tokensOut: number | null
  cacheRead: number | null
  cacheWrite: number | null
  cost: number | null
  /** Every reply reported a cost: otherwise `cost` covers only some of the turn's replies. */
  costComplete: boolean
  context: ContextReport | null
  memory: MemoryUse | null
  controls: AppliedControl[]
  /** The oldest loaded turn while older messages are unloaded: its start may be on an older page. */
  partial: boolean
  /** Cost of the loaded turns up to and including this one; null while no turn reported a cost. */
  runningCost: number | null
  /** An earlier (or this) turn reported no cost: the running total is a floor ("at least"). */
  runningFloor: boolean
}

/** A total over the turns that reported the value: `reported` of the ledger's turns did. */
export interface Total {
  value: number | null
  reported: number
}

export interface Ledger {
  turns: LedgerTurn[]
  /** Calls the harness couldn't tie to a turn (`association: 'unknown'`). */
  unplaced: RecordedStep[]
  totals: {
    turns: number
    toolCalls: number
    failedCalls: number
    tokensIn: Total
    tokensOut: Total
    cost: Total
  }
  /** Every page is loaded: the totals are the session's, not a loaded subtotal. */
  complete: boolean
  /** Any turn carries a preview record (context, memory or a control). */
  hasPreview: boolean
}

const isTime = (v: unknown): v is string => typeof v === 'string' && !Number.isNaN(Date.parse(v))
const count = (v: unknown): number | null =>
  typeof v === 'number' && Number.isFinite(v) && v >= 0 ? v : null
const money = (v: unknown): number | null => {
  const n = typeof v === 'string' && v.trim() !== '' ? Number(v) : v
  return typeof n === 'number' && Number.isFinite(n) && n >= 0 ? n : null
}
const codingAgent = (m: ChatMessage) => codingAgentOf(m.metadata)

/** The recorded decisions on one message; anything without a time, a rule's id and name, and an outcome is dropped. */
export function controlsOf(m: ChatMessage): AppliedControl[] {
  const raw = codingAgent(m)?.controls
  if (!Array.isArray(raw)) return []
  const out: AppliedControl[] = []
  for (const [i, c] of raw.entries()) {
    if (!isObject(c) || !isTime(c.at) || !isObject(c.rule)) continue
    const { id, name } = c.rule
    if (typeof id !== 'string' || !id || typeof name !== 'string' || !name.trim()) continue
    if (!(CONTROL_KINDS as readonly unknown[]).includes(c.kind)) continue
    if (!(CONTROL_OUTCOMES as readonly unknown[]).includes(c.outcome)) continue
    out.push({
      key: `${m.id}:${i}`,
      at: c.at,
      kind: c.kind as ControlKind,
      rule: { id, name },
      outcome: c.outcome as ControlOutcome,
      tool: typeof c.tool === 'string' && c.tool ? c.tool : undefined,
      tokensSaved: count(c.tokens_saved) ?? undefined,
    })
  }
  return out
}

const contextOf = (m: ChatMessage): ContextReport | null => {
  const raw = codingAgent(m)?.context
  return isObject(raw) ? contextReport(raw) : null
}

const memoryOf = (m: ChatMessage): MemoryUse | null => {
  const raw = codingAgent(m)?.memory
  if (!isObject(raw)) return null
  const tokens = count(raw.tokens)
  return tokens === null ? null : { tokens, items: count(raw.items) }
}

/** Sum the reported values; null when none was reported (never 0 for "not reported"). */
function sum(values: (number | null)[]): number | null {
  let total: number | null = null
  for (const v of values) if (v !== null) total = (total ?? 0) + v
  return total
}

/** `complete[i]` false counts turn i as not reported even when part of it was (its value is still summed). */
const total = (values: (number | null)[], complete?: boolean[]): Total => ({
  value: sum(values),
  reported: values.filter((v, i) => v !== null && (complete?.[i] ?? true)).length,
})

/**
 * Every loaded message once, oldest first. Pages arrive newest first and each is in the server's order (timestamp,
 * then id, to the microsecond), so joining them oldest page first keeps that order; a refetch can repeat rows.
 */
export function mergeMessages(pages: readonly MessagesPage[]): ChatMessage[] {
  const seen = new Set<string>()
  const out: ChatMessage[] = []
  for (const p of [...pages].reverse())
    for (const m of p.data)
      if (!seen.has(m.id)) {
        seen.add(m.id)
        out.push(m)
      }
  return out
}

/** Messages grouped into turns, oldest first. */
export function groupTurns(messages: readonly ChatMessage[]): ChatMessage[][] {
  const turns: ChatMessage[][] = []
  const byTurnId = new Map<string, ChatMessage[]>()
  let current: { msgs: ChatMessage[]; id: string | null } | null = null
  for (const m of messages) {
    if (m.role === 'system') continue
    const tid = m.external_turn_id || null
    const known = tid ? byTurnId.get(tid) : undefined
    if (known) {
      known.push(m)
      continue
    }
    // A new turn: the first message, any prompt not already in a known turn, or a reply naming another turn. A reply
    // with an id joins a turn that has none yet (its prompt was saved without one) and names it.
    const starts =
      !current || m.role === 'user' || (tid !== null && current.id !== null && tid !== current.id)
    if (starts || !current) {
      current = { msgs: [m], id: tid }
      turns.push(current.msgs)
    } else {
      current.msgs.push(m)
      current.id ??= tid
    }
    if (tid) byTurnId.set(tid, current.msgs)
  }
  return turns
}

export function buildLedger(
  pages: readonly MessagesPage[],
  opts: { complete: boolean; preview: boolean },
): Ledger {
  const groups = groupTurns(mergeMessages(pages))
  const unplaced: RecordedStep[] = []
  let running: number | null = null
  let floor = false
  let hasPreview = false
  const turns = groups.map((msgs, i): LedgerTurn => {
    const [first] = msgs as [ChatMessage, ...ChatMessage[]]
    const user = msgs.find((m) => m.role === 'user') ?? null
    const replies = msgs.filter((m) => m.role === 'assistant')
    const tools: RecordedStep[] = []
    for (const r of replies)
      for (const s of toolCallsToSteps(r.metadata)?.steps ?? [])
        // Keyed by message too: a harness repeats call ids (or has none) across messages.
        (s.association === 'unknown' ? unplaced : tools).push({ ...s, key: `${r.id}:${s.key}` })
    const costs = replies.map((r) => money(r.cost_usd))
    const cost = sum(costs)
    if (cost !== null) running = (running ?? 0) + cost
    if (!replies.length || costs.some((c) => c === null)) floor = true
    const cacheRead = sum(replies.map((r) => count(r.cache_read_tokens)))
    const cacheWrite = sum(replies.map((r) => count(r.cache_creation_tokens)))
    const fresh = sum(replies.map((r) => count(r.input_tokens)))
    const context = opts.preview ? (replies.map(contextOf).find(Boolean) ?? null) : null
    const memory = opts.preview ? (replies.map(memoryOf).find(Boolean) ?? null) : null
    const controls = opts.preview ? replies.flatMap(controlsOf) : []
    if (context || memory || controls.length) hasPreview = true
    return {
      key: msgs.find((m) => m.external_turn_id)?.external_turn_id ?? first.id,
      n: i + 1,
      at: first.timestamp,
      prompt: user?.content ?? null,
      tools,
      tokensIn: sum([fresh, cacheRead, cacheWrite]),
      tokensOut: sum(replies.map((r) => count(r.output_tokens))),
      cacheRead,
      cacheWrite,
      cost,
      costComplete: replies.length > 0 && costs.every((x) => x !== null),
      context,
      memory,
      controls,
      partial: !opts.complete && i === 0 && !user,
      runningCost: running,
      runningFloor: floor && running !== null,
    }
  })
  const calls = turns.flatMap((t) => t.tools).concat(unplaced)
  return {
    turns,
    unplaced,
    totals: {
      turns: turns.length,
      toolCalls: calls.length,
      failedCalls: calls.filter((s) => s.status === 'error').length,
      tokensIn: total(turns.map((t) => t.tokensIn)),
      tokensOut: total(turns.map((t) => t.tokensOut)),
      cost: total(
        turns.map((t) => t.cost),
        turns.map((t) => t.costComplete),
      ),
    },
    complete: opts.complete,
    hasPreview,
  }
}

export interface BudgetLine {
  budget: SessionBudget
  /** Share of the limit this session used, 0-100+. */
  sessionPct: number
  /** Share used before the session started; null when not recorded. */
  beforePct: number | null
}

const PRECEDENCE: SessionBudget['scope'][] = ['session', 'agent', 'owner']

/** The budgets in precedence order (session → agent → owner), with the share each has used. */
export function budgetLines(budgets: readonly SessionBudget[]): BudgetLine[] {
  return [...budgets]
    .sort((a, b) => PRECEDENCE.indexOf(a.scope) - PRECEDENCE.indexOf(b.scope))
    .map((budget) => ({
      budget,
      sessionPct: (budget.session_used_usd / budget.limit_usd) * 100,
      beforePct:
        budget.used_before_usd === null ? null : (budget.used_before_usd / budget.limit_usd) * 100,
    }))
}

/**
 * The tightest budget: the highest share used by the end of the session. A budget whose earlier use isn't recorded is
 * compared on the session's own share. Ties keep precedence (session, agent, owner).
 */
export function tightest(lines: readonly BudgetLine[]): BudgetLine | null {
  let best: BudgetLine | null = null
  for (const l of lines) {
    const pct = (l.beforePct ?? 0) + l.sessionPct
    const bestPct = best ? (best.beforePct ?? 0) + best.sessionPct : -1
    if (pct > bestPct) best = l
  }
  return best
}

/** The share of a budget used by the end of each turn, keyed by turn id: before + this session's turns so far. */
export function runningShare(line: BudgetLine): Map<string, number> {
  const out = new Map<string, number>()
  let used = line.budget.used_before_usd ?? 0
  for (const t of line.budget.turns) {
    used += t.used_usd
    out.set(t.turn_id, (used / line.budget.limit_usd) * 100)
  }
  return out
}

/** The CSV's words (from `copy.ledger.csv`, so the export reads like the page). */
export interface CsvLabels {
  headers: Record<
    | 'turn'
    | 'time'
    | 'tools'
    | 'tokensIn'
    | 'cacheRead'
    | 'cacheWrite'
    | 'tokensOut'
    | 'context'
    | 'memory'
    | 'controls'
    | 'source'
    | 'coverage',
    string
  >
  cost: string
  notReported: string
  notRecorded: string
  source: { recorded: string; preview: string }
  coverage: (complete: boolean, turns: number) => string
}

interface LedgerCsvRow {
  turn: LedgerTurn | null
  /** The calls under "Turn not recorded" share one row. */
  unplaced?: RecordedStep[]
}

/**
 * The ledger's CSV (X4): turn, time (UTC), tool names, tokens in (with cache reads and writes apart), tokens out, cost,
 * context tokens, memory tokens, controls applied, source, coverage. A value the server didn't report is the literal
 * "not reported"; every row says whether the file holds the whole session or only the loaded turns (a file travels
 * without the page's note). Tool arguments, results, prompts and replies are never exported (CEO decision 5): only
 * names and recorded statuses. Formula escaping is `toCsv`'s.
 */
export function ledgerCsv(ledger: Ledger, l: CsvLabels): string {
  const h = l.headers
  const nr = <T>(v: T | null | undefined, f: (x: T) => string | number) =>
    v === null || v === undefined ? l.notReported : f(v)
  const names = (steps: RecordedStep[]) =>
    steps.map((s) => (s.statusWord ? `${s.name} (${s.recordedStatus})` : s.name)).join('; ')
  const of = (f: (t: LedgerTurn) => string | number) => (r: LedgerCsvRow) =>
    r.turn ? f(r.turn) : ''
  const coverage = l.coverage(ledger.complete, ledger.turns.length)
  const columns: CsvColumn<LedgerCsvRow>[] = [
    { header: h.turn, value: (r) => (r.turn ? r.turn.n : l.notRecorded) },
    { header: h.time, value: of((t) => t.at) },
    { header: h.tools, value: (r) => names(r.turn ? r.turn.tools : (r.unplaced ?? [])) },
    { header: h.tokensIn, value: of((t) => nr(t.tokensIn, (v) => v)) },
    { header: h.cacheRead, value: of((t) => nr(t.cacheRead, (v) => v)) },
    { header: h.cacheWrite, value: of((t) => nr(t.cacheWrite, (v) => v)) },
    { header: h.tokensOut, value: of((t) => nr(t.tokensOut, (v) => v)) },
    { header: l.cost, value: of((t) => nr(t.cost, (v) => v)) },
    { header: h.context, value: of((t) => nr(t.context, (c) => c.kept.tokens)) },
    { header: h.memory, value: of((t) => nr(t.memory, (m) => m.tokens)) },
    {
      header: h.controls,
      value: of((t) =>
        t.controls.length
          ? t.controls.map((c) => `${c.rule.name}: ${c.outcome}`).join('; ')
          : l.notReported,
      ),
    },
    {
      header: h.source,
      value: (r) =>
        r.turn && (r.turn.context || r.turn.memory || r.turn.controls.length)
          ? l.source.preview
          : l.source.recorded,
    },
    { header: h.coverage, value: () => coverage },
  ]
  const rows: LedgerCsvRow[] = ledger.turns.map((turn) => ({ turn }))
  if (ledger.unplaced.length) rows.push({ turn: null, unplaced: ledger.unplaced })
  return toCsv(columns, rows)
}
