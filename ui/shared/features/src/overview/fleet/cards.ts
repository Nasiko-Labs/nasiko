/**
 * A Map card's insights (pure): what the drawer says when a card is picked (Ans, 2026-10-17: a click on a card lights
 * it, fades the rest and opens its insights in the right-hand drawer first; the drawer's buttons go to the pages). Each
 * card has its one line of what it adds up to, how to read its colours, which parts are in it (they light their lines),
 * which overview panel its problems come from, and the pages to go on to.
 *
 * The board's own selections beside the estate's grammar (map/selection.ts): `card:<id>` for a card and `day:<index>`
 * for one of the bill's days. They are raw `sel` values, like the village's `place:…`.
 */
import type { OptimisationView } from '@nasiko/features/tokenops/optimisation'
import { fmtMoney } from '@nasiko/ui/lib/format'
import type { PanelId } from '../estate/types'
import type { EstateView } from '../estate/useEstateView'
import type { Hud } from '../village/model'
import { boardCopy as copy } from './copy'
import { isCardId, type CardId } from './layout'
import { BILL, BUDGET, kindId, ORG, TEAMLESS } from './lines'
import { uses, type Board, type SpendCard } from './model'

/** Where a card's buttons go: the pages the board hands off to. */
export type To =
  | '/agents'
  | '/deploy'
  | '/workflows'
  | '/workflows/new'
  | '/workflows/runs'
  | '/mcp'
  | '/harnesses'
  | '/router'
  | '/tokenops'
  | '/optimization'
export interface Action {
  label: string
  to: To
  hash?: string
}
export type Swatches = readonly (readonly [string, string])[]

export interface CardInsight {
  title: string
  line: string
  key: string
  swatches?: Swatches
  /** The overview panel whose problems are this card's ("Needs a look"). */
  panel: PanelId | null
  actions: Action[]
}

export type BoardSel = { type: 'card'; id: CardId } | { type: 'day'; index: number }

/** The board's own selection in a raw `sel`, or null for anything else. */
export function boardSel(sel: string | undefined): BoardSel | null {
  if (sel?.startsWith('card:')) {
    const id = sel.slice('card:'.length)
    return isCardId(id) ? { type: 'card', id } : null
  }
  if (sel?.startsWith('day:')) {
    const n = Number(sel.slice('day:'.length))
    return Number.isInteger(n) && n >= 0 ? { type: 'day', index: n } : null
  }
  return null
}

/** The card a board selection lights: the card itself, or the bill for one of its days. */
export const cardOf = (s: BoardSel | null): CardId | null =>
  !s ? null : s.type === 'card' ? s.id : 'bill'

const A = copy.actions
const BUDGETS: Action = { label: A.setBudgets, to: '/router', hash: 'router-budgets' }
const TONE_KEY: Swatches = [
  ['t-ok', copy.tone.ok],
  ['t-warn', copy.tone.warn],
  ['t-bad', copy.tone.bad],
  ['t-idle', copy.tone.idle],
]

/** The opportunities this scope can act on: all of them in Org, those that touch your parts in Mine. */
export function visibleOpps(v: EstateView) {
  return v.estate.opportunities
    .filter((o) => v.view.scope === 'org' || o.entities.some((x) => v.mine.has(x)))
    .sort((a, b) => b.saving_usd - a.saving_usd)
}

/** The parts a card holds: their lines light with the card. */
export function cardParts(id: CardId, board: Board, v: EstateView): Set<string> {
  switch (id) {
    case 'teams':
      return new Set([ORG, ...board.teams.map((t) => t.id ?? TEAMLESS)])
    case 'agents':
      return new Set(board.agents.map((a) => a.id))
    case 'workflows':
      return new Set(board.workflows.map((w) => w.id))
    case 'tools':
      return new Set(board.tools.map((t) => t.id))
    case 'routers':
      return new Set(board.routers.map((r) => r.id))
    case 'providers':
      return new Set(board.providers.flatMap((p) => [p.id, ...p.models.map((m) => m.id)]))
    case 'harness':
      return new Set(board.harnesses.map((c) => kindId(c.key)))
    case 'bill':
      return new Set([BILL])
    case 'budget':
      return new Set([BUDGET])
    case 'opportunities':
      return new Set(visibleOpps(v).flatMap((o) => o.entities))
    case 'saved':
      return new Set()
  }
}

/** A card's insights, for the drawer. */
export function cardInsight(
  id: CardId,
  v: EstateView,
  board: Board,
  card: SpendCard,
  hud: Hud,
  o: OptimisationView | undefined,
  mine: boolean,
): CardInsight {
  const e = v.estate
  const d = copy.detail
  const sum = (xs: readonly number[]) => xs.reduce((n, x) => n + x, 0)
  switch (id) {
    case 'teams': {
      const named = board.teams.filter((t) => t.id)
      const p = copy.peopleKey
      return {
        title: copy.teams,
        line: d.teams(
          named.length,
          named.filter((t) => (t.pace ?? 0) > 1).length,
          named.filter((t) => !t.limit).length,
        ),
        key: d.teamsKey,
        swatches: [
          ['t-ok', p.ok],
          ['t-warn', p.warn],
          ['t-bad', p.bad],
          ['t-idle', p.idle],
        ],
        panel: 'people',
        actions: [BUDGETS],
      }
    }
    case 'agents':
      return {
        title: copy.agents,
        line: d.agents(
          board.agentCount,
          sum(e.agents.map((a) => a.spend_usd)),
          board.agents.filter((a) => a.now === 'bad').length,
        ),
        key: d.agentsKey,
        swatches: TONE_KEY,
        panel: 'agents',
        actions: [
          { label: A.agents, to: '/agents' },
          { label: A.deploy, to: '/deploy' },
        ],
      }
    case 'harness':
      return {
        title: copy.harness,
        line: d.harness(
          board.harnessCount,
          new Set(e.harnesses.map((h) => h.harness)).size,
          sum(e.harnesses.map((h) => h.spend_usd ?? 0)),
        ),
        key: d.harnessKey,
        swatches: [
          ['h-high', copy.harnessTone.high],
          ['h-low', copy.harnessTone.low],
          ['h-empty', copy.harnessTone.empty],
          ['h-bad', copy.harnessTone.bad],
        ],
        panel: 'harnesses',
        actions: [{ label: A.harnesses, to: '/harnesses' }],
      }
    case 'workflows':
      return {
        title: copy.workflows,
        line: d.workflows(
          board.workflowCount,
          sum(e.workflows.map((w) => w.runs)),
          board.workflows.filter((w) => w.failing).length,
        ),
        key: d.workflowsKey,
        swatches: TONE_KEY,
        panel: 'workflows',
        actions: [
          { label: A.runs, to: '/workflows/runs' },
          { label: A.newWorkflow, to: '/workflows/new' },
        ],
      }
    case 'tools':
      return {
        title: copy.tools,
        line: d.tools(
          board.toolCount,
          board.tools.filter((t) => uses(t) === 0).length,
          board.tools.filter((t) => t.offSince).length,
        ),
        key: d.toolsKey,
        panel: 'tools',
        actions: [
          { label: A.tools, to: '/mcp' },
          { label: A.connect, to: '/mcp' },
        ],
      }
    case 'routers':
      return {
        title: copy.routers,
        line: d.routers(board.routerCount, hud.routedShare),
        key: d.routersKey,
        panel: 'models',
        actions: [{ label: A.routers, to: '/router' }],
      }
    case 'providers': {
      const top = [...e.models].sort((a, b) => b.spend_usd - a.spend_usd)[0]
      return {
        title: copy.providers,
        line: d.providers(
          top && top.spend_usd > 0 ? `${top.short || top.name}, ${fmtMoney(top.spend_usd)}` : null,
        ),
        key: d.providersKey,
        panel: 'models',
        actions: [{ label: A.providers, to: '/router' }],
      }
    }
    case 'bill': {
      const s = card.series
      return {
        title: copy.totalSpend,
        line: d.bill(s.length ? sum(s) / s.length : 0, Math.max(0, ...s), card.threshold),
        key: `${d.billKey}. ${d.paying}.`,
        panel: null,
        actions: [
          { label: A.tokenops, to: '/tokenops' },
          { label: A.investigate, to: '/tokenops' },
        ],
      }
    }
    case 'saved':
      return {
        title: copy.saved,
        line: d.saved(o?.costSavedPct ?? null, o?.callsOptimised ?? 0, o?.callsInWindow ?? 0),
        key: d.savedKey,
        panel: null,
        actions: [{ label: A.optimize, to: '/optimization' }],
      }
    case 'opportunities': {
      const opps = visibleOpps(v)
      return {
        title: copy.opps.title,
        line: d.opps(opps.length, sum(opps.map((x) => x.saving_usd))),
        key: d.oppsKey,
        panel: null,
        actions: [{ label: A.optimize, to: '/optimization' }],
      }
    }
    case 'budget':
      return {
        title: mine ? copy.allowance : copy.budget,
        line: d.budget(hud.unbudgeted, hud.uncapped),
        key: d.budgetKey,
        panel: null,
        actions: [BUDGETS],
      }
  }
}
