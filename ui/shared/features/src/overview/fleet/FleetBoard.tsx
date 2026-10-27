/**
 * The Overview's Map view: the "Fleet Runtime / TokenOps" board (the user's design, 2026-10-16, laid out by Ans's
 * wireframes of 2026-10-17), drawn as a world inside the village's canvas (VillageCanvas hosts both, so the HUD, the
 * drawer and pan and zoom are the same in every view). Org on the left (every team against its budget, its people as
 * tiles). Fleet Runtime in the middle: agents and workflows on top (workflows in the Teams card's row shape); under the
 * agents the LLM routers (a small box of tiles, a dot on one that is down) over the providers' models (a bar each, on
 * one scale); under the workflows the connectors and tools (their marks only, most used first, a recently turned-off
 * one faded); the local harnesses across the bottom. TokenOps on the right: the bill (the range's spend added up by
 * provider and by source, to one total, with its daily line), the savings globe, the opportunities and the month's
 * budget. Token lines join the parts the way the village's coin trails do (lines.ts), drawn under the cards and routed
 * through the gaps: the harnesses run straight to the bill, and the budget pays each provider.
 *
 * The board is HTML at its own coordinates (layout.ts); the canvas's viewport sets the lines' SVG viewBox and the
 * board's transform together, so the two always line up. Picking a part puts `node:<id>` in the URL: it fills like the
 * sidebar's current page, what it touches stays lit with its lines, the rest dims, and the canvas's drawer explains it.
 * A click on a card anywhere but a part (or on its title) picks the card (`card:<id>`): a light ring on it, the other
 * cards faded, its lines lit, and its insights in the drawer, whose buttons go on to the pages (cards.ts). Nothing on
 * the board navigates by itself. A peak dot on the bill's line picks that day (`day:<index>`). Hovering the bill turns
 * every line it pays for green; red stays where spend is going up.
 *
 * Colour means state, in the Teams card's tones (green, orange, red, grey) everywhere on the board; brand colours
 * appear only on the providers' marks and models. Every mark (provider, router, tool) is 16 px.
 *
 * Conditional classes go through `cn()`: Prettier's Tailwind plugin trims the leading space of a class string, which
 * would glue `fb-x` and `fb-y` together in a template literal.
 */
import {
  createContext,
  use,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from 'react'
import { Button } from '@nasiko/ui/primitives/button'
import { fmtMoney, fmtShortDay } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import { estateCopy } from '../estate/copy'
import type { EstateView } from '../estate/useEstateView'
import { BRAND_PATHS } from '../map/brands'
import type { ViewState } from '../map/useViewport'
import type { Hud } from '../village/model'
import { boardSel, cardOf, cardParts, visibleOpps } from './cards'
import { boardCopy as copy } from './copy'
import { BOARD, CARDS, SECTIONS, TITLE_TOP, type Box, type CardId } from './layout'
import {
  BILL,
  BUDGET,
  buildLines,
  focusOf,
  kindId,
  litLine,
  ORG,
  OVER,
  pointsOf,
  roundPath,
  TEAMLESS,
  type Anchor,
  type Line,
} from './lines'
import {
  type Bill,
  type Board,
  type HarnessCol,
  type ProviderRow,
  type RouterCard,
  type SpendCard,
  type TeamRow,
  type Tone,
  type ToolRow,
  type WorkflowRow,
  uses,
} from './model'
import './board.css'

export { BOARD } from './layout'

/** The month's budget reads orange once it passes this share of the limit (its tick on the gauge). */
const ALERT = 0.8

interface Props {
  v: EstateView
  board: Board
  /** The bill's daily line (model.ts `billSeries`). */
  card: SpendCard
  /** The UTC day of each of its points. */
  dates: readonly string[]
  /** The canvas's numbers: the month's budget (the viewer's allowance in Mine) and what nothing governs. */
  hud: Hud
  mine: boolean
  saved: number | null
  sel: string | undefined
  onSelect(sel: string): void
  onClear(): void
  /** A click on open board, off every card. */
  onBackground(): void
  /** The canvas's viewport: the board follows it. */
  subscribe(l: (s: ViewState) => void): () => void
  viewNow(): ViewState
  /** Tokens move along the lines (motion on, token flow on). */
  running: boolean
  /** Where each part is on the board, once measured: the canvas centres on one from it. */
  onAnchors(at: ReadonlyMap<string, Anchor>): void
}

type Hit = (id: string) => void
/** A part's selection class: `fb-sel`, `fb-rel` (lit with it), `fb-dim`, or none. */
type Lit = (ids: string | readonly string[]) => string | undefined

/** The picked card (lit, the rest faded) and how a card asks to be picked: every Card reads it. */
const Pick = createContext<{ card: CardId | null; onCard(id: CardId): void }>({
  card: null,
  onCard: () => {},
})

/** An element's box in the board's own units: offsets ignore the viewport's transform. */
function boxIn(el: HTMLElement, root: HTMLElement): Anchor {
  let x = 0
  let y = 0
  let n: HTMLElement | null = el
  while (n && n !== root) {
    x += n.offsetLeft
    y += n.offsetTop
    n = n.offsetParent as HTMLElement | null
  }
  return { x, y, w: el.offsetWidth, h: el.offsetHeight }
}

export function FleetBoard({
  v,
  board,
  card,
  dates,
  hud,
  mine,
  saved,
  sel,
  onSelect,
  onClear,
  onBackground,
  subscribe,
  viewNow,
  running,
  onAnchors,
}: Props) {
  const e = v.estate
  const lines = useMemo(() => buildLines(v, board), [v, board])
  const nodeId = sel?.startsWith('node:') ? sel.slice('node:'.length) : null
  const node = nodeId && v.nodes.has(nodeId) ? nodeId : null
  const oppId = sel?.startsWith('opp:') ? sel.slice('opp:'.length) : null
  const opp = oppId ? e.opportunities.find((o) => o.id === oppId) : undefined
  const picked = boardSel(sel)
  const pickedCard = cardOf(picked)
  const day = picked?.type === 'day' ? picked.index : null
  // The budget's gauge opens the token battery: the board lights the org and its teams for it.
  const tank = sel === 'place:tank'
  const selKey = node ?? (tank ? ORG : null)
  const focus = useMemo(
    () =>
      node
        ? focusOf(v, node)
        : tank
          ? new Set([ORG, ...board.teams.map((t) => t.id ?? TEAMLESS)])
          : opp
            ? new Set(opp.entities)
            : null,
    [v, node, tank, opp, board.teams],
  )
  // A picked card lights the lines of the parts in it.
  const parts = useMemo(
    () => (pickedCard ? cardParts(pickedCard, board, v) : null),
    [pickedCard, board, v],
  )
  // The ways to save that name the selection light the opportunities; a router carries the savings themselves.
  const named = node ? e.opportunities.some((o) => o.entities.includes(node)) : false
  const savingsLit = !focus
    ? undefined
    : (node && v.nodes.get(node)?.kind === 'router') || named || opp
      ? 'fb-rel'
      : 'fb-dim'
  // Hovering the bill: every line it pays for turns green, red stays red.
  const [paying, setPaying] = useState(false)

  const outer = useRef<HTMLDivElement>(null)
  const root = useRef<HTMLDivElement>(null)
  /** The lines drawn over the cards: the org's, inside the Teams card. */
  const over = useRef<HTMLDivElement>(null)
  // The board and its overlay follow the viewport: the same view the lines' viewBox shows.
  useLayoutEffect(() => {
    const els = [root.current, over.current]
    const put = ({ x, y, z }: ViewState) => {
      const t = `translate(${(-x * z).toFixed(2)}px, ${(-y * z).toFixed(2)}px) scale(${z})`
      for (const el of els) if (el) el.style.transform = t
    }
    put(viewNow())
    return subscribe(put)
  }, [subscribe, viewNow])

  // Where each part sits, measured once it is laid out (and again once the fonts are in).
  const [at, setAt] = useState<ReadonlyMap<string, Anchor>>(() => new Map())
  useLayoutEffect(() => {
    const el = root.current
    if (!el) return
    const measure = () => {
      if (!el.offsetWidth) return
      const m = new Map<string, Anchor>()
      for (const x of el.querySelectorAll<HTMLElement>('[data-anchor]'))
        if (x.dataset.anchor) m.set(x.dataset.anchor, boxIn(x, el))
      onAnchors(m)
      setAt(m)
    }
    measure()
    let live = true
    void document.fonts?.ready.then(() => {
      if (live) measure()
    })
    return () => {
      live = false
    }
  }, [board, onAnchors])

  /** A card picked again clears it. */
  const onCard = useMemo(
    () => (id: CardId) => (sel === `card:${id}` ? onClear() : onSelect(`card:${id}`)),
    [sel, onClear, onSelect],
  )
  // A click on a card (not on a part or a button) picks it; anywhere else off the cards clears.
  useLayoutEffect(() => {
    const el = outer.current
    if (!el) return
    const click = (ev: MouseEvent) => {
      const t = ev.target as Element
      if (t.closest('[data-node],a,button,[data-keep]')) return
      const id = t.closest<HTMLElement>('[data-card]')?.dataset.card
      if (id && id in CARDS) onCard(id as CardId)
      else onBackground()
    }
    el.addEventListener('click', click)
    return () => el.removeEventListener('click', click)
  }, [onCard, onBackground])

  const hit: Hit = (id) => (selKey === id ? onClear() : onSelect(`node:${id}`))
  const lit: Lit = (ids) => {
    const list = typeof ids === 'string' ? [ids] : ids
    if (selKey && list.includes(selKey)) return 'fb-sel'
    if (!focus) return undefined
    return list.some((id) => focus.has(id)) ? 'fb-rel' : 'fb-dim'
  }
  /** A container only dims: the part in it carries the fill. */
  const dim: Lit = (ids) => {
    const list = typeof ids === 'string' ? [ids] : ids
    return focus && !list.some((id) => focus.has(id) || id === selKey) ? 'fb-dim' : undefined
  }
  const pick = useMemo(() => ({ card: pickedCard, onCard }), [pickedCard, onCard])
  const lineProps = { lines, at, selKey, focus, parts, running }

  return (
    <div ref={outer} className={cn('fb', paying && 'fb-paying')} data-testid="overview-canvas">
      <Lines {...lineProps} layer="under" />
      <Pick value={pick}>
        <div
          ref={root}
          className={cn('fb-frame', focus && 'fb-focus', pickedCard && 'fb-picking')}
          style={{ width: BOARD.width, height: BOARD.height }}
          role="group"
          aria-label={copy.label}
        >
          {v.fallback ? (
            <p className="fb-note" style={{ left: CARDS.teams.left, top: TITLE_TOP - 36 }}>
              {estateCopy.fallback.note}
            </p>
          ) : null}
          {SECTIONS.map((s) => (
            <h2 key={s.id} className="fb-h" style={{ left: s.left, top: TITLE_TOP }}>
              {copy[s.id]}
            </h2>
          ))}
          <Legend />
          <Teams board={board} hit={hit} lit={lit} />
          <Agents board={board} hit={hit} lit={lit} />
          <Workflows board={board} hit={hit} lit={lit} />
          <Routers routers={board.routers} all={board.routerCount} hit={hit} lit={lit} />
          <Providers
            providers={board.providers}
            models={e.models.length}
            all={e.providers.length}
            hit={hit}
            lit={lit}
            dim={dim}
          />
          <Tools
            rows={board.tools}
            all={board.toolCount}
            asOf={Date.parse(e.as_of)}
            hit={hit}
            lit={lit}
          />
          <Harnesses
            cols={board.harnesses}
            all={board.harnessCount}
            hit={hit}
            lit={lit}
            dim={dim}
          />
          <BillCard
            bill={board.bill}
            card={card}
            dates={dates}
            day={day}
            saved={saved}
            hit={hit}
            lit={lit}
            onDay={(i) => (day === i ? onSelect('card:bill') : onSelect(`day:${i}`))}
            onPaying={setPaying}
          />
          <Saved saved={saved} lit={savingsLit} />
          <Opportunities
            v={v}
            sel={oppId}
            onPick={(id) => (oppId === id ? onClear() : onSelect(`opp:${id}`))}
            lit={savingsLit}
          />
          <Budget
            hud={hud}
            mine={mine}
            selected={tank}
            lit={lit}
            onOpen={() => (tank ? onClear() : onSelect('place:tank'))}
          />
        </div>
      </Pick>
      <div ref={over} className="fb-over" style={{ width: BOARD.width, height: BOARD.height }}>
        <Lines {...lineProps} layer="over" />
      </div>
    </div>
  )
}

/**
 * The token lines, under the cards (`under`) or, for the ones that run inside a card, over them (`over`): dimmed
 * ones first, so what a selection lights draws on top.
 */
function Lines({
  lines,
  at,
  selKey,
  focus,
  parts,
  running,
  layer,
}: {
  lines: Line[]
  at: ReadonlyMap<string, Anchor>
  selKey: string | null
  focus: ReadonlySet<string> | null
  /** A picked card's parts: their lines are lit, the rest are not. */
  parts: ReadonlySet<string> | null
  running: boolean
  layer: 'under' | 'over'
}) {
  const drawn = useMemo(() => {
    const max = Math.max(1, ...lines.map((l) => l.usd))
    return lines.flatMap((l) => {
      if (OVER.has(l.route) !== (layer === 'over')) return []
      const pts = pointsOf(l, at)
      if (!pts || pts.length < 2) return []
      const w = l.kind === 'spend' ? 1.2 + 3.3 * Math.sqrt(l.usd / max) : 1
      return [{ l, d: roundPath(pts), w }]
    })
  }, [lines, at, layer])
  const on = (l: Line) =>
    parts
      ? parts.has(l.from) || parts.has(l.to)
      : focus && selKey
        ? litLine(l, selKey, focus)
        : focus
          ? focus.has(l.from) || focus.has(l.to)
          : null
  // Lit over unlit, red over blue (where spend goes up shows on a shared stretch), spend over calls.
  const rank = (l: Line) =>
    Number(on(l) === true) * 4 + Number(l.hot) * 2 + Number(l.kind === 'spend')
  const order = [...drawn].sort((a, b) => rank(a.l) - rank(b.l))
  return (
    // The under layer is the zone's first svg, so the viewport writes its viewBox; the over layer sits in the overlay,
    // which the viewport transforms, so it draws in board units.
    <svg
      className={cn('fb-lines', layer === 'over' && 'fb-lines-over')}
      viewBox={layer === 'over' ? `0 0 ${BOARD.width} ${BOARD.height}` : undefined}
      aria-hidden
    >
      {order.map(({ l, d, w }) => {
        const o = on(l)
        const cls = cn(l.kind, l.hot && 'hot', o === true && 'on', o === false && 'off')
        return (
          <g key={l.id} data-line={l.id}>
            <path d={d} className={cn('fb-ln', cls)} strokeWidth={w} />
            {running && (o === true || (o === null && l.kind === 'spend')) ? (
              <path d={d} className={cn('fb-run', cls)} strokeWidth={w + 1.4} />
            ) : null}
          </g>
        )
      })}
    </svg>
  )
}

/** What the lines mean, under the Org. */
function Legend() {
  const items = [
    ['spend', copy.legend.spend],
    ['spend hot', copy.legend.hot],
    ['calls', copy.legend.calls],
  ] as const
  return (
    <p
      className="fb-legend"
      style={{ left: CARDS.teams.left, top: CARDS.teams.top + CARDS.teams.height + 24 }}
    >
      {items.map(([cls, label]) => (
        <span key={cls}>
          <svg width={22} height={6} aria-hidden>
            <line x1={1} y1={3} x2={21} y2={3} className={cn('fb-ln on', cls)} strokeWidth={2.5} />
          </svg>
          {label}
        </span>
      ))}
    </p>
  )
}

/**
 * A card: its title (a button that picks the card) and count over its body. Picked, it carries a light ring and the
 * other cards fade; `anchor` is where lines land on the card itself (the bill, the budget).
 */
function Card({
  id,
  title,
  box,
  count,
  children,
  className,
  anchor,
  inline,
  hover,
}: {
  id: CardId
  title: string
  box?: Box
  count?: ReactNode
  children: ReactNode
  className?: string
  anchor?: string
  /** The title beside the body, not over it (the routers' one row of tiles). */
  inline?: boolean
  hover?: (on: boolean) => void
}) {
  const h = useId()
  const { card, onCard } = use(Pick)
  const on = card === id
  return (
    <section
      className={cn(
        'fb-card',
        inline && 'fb-inline',
        on && 'fb-cardsel',
        card && !on && 'fb-cardfade',
        className,
      )}
      style={box ?? CARDS[id]}
      aria-labelledby={h}
      data-card={id}
      data-anchor={anchor}
      onPointerEnter={hover ? () => hover(true) : undefined}
      onPointerLeave={hover ? () => hover(false) : undefined}
    >
      <div className="fb-title">
        <h3 id={h}>
          <Button
            variant="ghost"
            className="fb-hit fb-cardpick"
            aria-label={copy.cardLabel(title)}
            aria-pressed={on}
            onClick={() => onCard(id)}
          >
            {title}
          </Button>
        </h3>
        {count}
      </div>
      {children}
    </section>
  )
}

function Count({ shown, all }: { shown: number; all: number }) {
  if (!all) return null
  return <span className="fb-count">{copy.count(shown, all)}</span>
}

const tip = (k: number, tone: string) => copy.day(k, copy.tone[tone] ?? tone)
const pct = (f: number) => `${(Math.max(0, Math.min(1, f)) * 100).toFixed(1)}%`

/** The share of a budget a team bar spans: up to 140% of it, so every team's line sits at the same place. */
const OVER_BUDGET = 1.4

function BudgetBar({ t, max }: { t: TeamRow; max: number }) {
  if (!t.limit) {
    return (
      <span className="fb-bbar none" aria-hidden>
        <i className="fb-bfill" style={{ width: pct(t.spent / max) }} />
      </span>
    )
  }
  const scale = t.limit * OVER_BUDGET
  const tone = (t.pace ?? 0) > 1 ? 'bad' : (t.pace ?? 0) > 0.9 ? 'warn' : 'ok'
  return (
    <span className="fb-bbar" aria-hidden>
      <i className={cn('fb-bfill', `b-${tone}`)} style={{ width: pct(t.spent / scale) }} />
      <i className="fb-btick" style={{ left: pct(1 / OVER_BUDGET) }} />
    </span>
  )
}

/** People shown in a team's row, at most: the rest are a count. */
const PEOPLE_SHOWN = 14

function Team({ t, max, hit, lit }: { t: TeamRow; max: number; hit: Hit; lit: Lit }) {
  const id = t.id ?? TEAMLESS
  const name = t.id ? t.name : copy.noTeam
  const over = (t.pace ?? 0) > 1
  const extra = t.people.length - PEOPLE_SHOWN
  const body = (
    <>
      <span className="fb-row">
        <span className="fb-name">{name}</span>
        <span className="fb-val">
          {copy.usd0(t.spent)}
          {t.limit ? <span className="fb-of"> / {copy.usd0(t.limit)}</span> : null}
        </span>
      </span>
      <BudgetBar t={t} max={max} />
      <span className="fb-trow">
        <span className="fb-people" aria-hidden>
          {t.people.slice(0, PEOPLE_SHOWN).map((p, i) => (
            // eslint-disable-next-line @eslint-react/no-array-index-key -- people are tiles without names, in order
            <span key={i} className={`t-${p}`} />
          ))}
          {extra > 0 ? <span className="fb-more">+{extra}</span> : null}
        </span>
        <span className={cn('fb-pace', over ? 'fb-red' : !t.limit && 'fb-warn')}>
          {t.limit && t.pace !== null ? copy.pace(t.pace) : copy.teamNoBudget}
        </span>
      </span>
    </>
  )
  const label = `${copy.teamLabel(name, t.spent, t.limit, t.pace)}. ${copy.people(t.people)}`
  if (!t.id)
    return (
      <div className={cn('fb-team', lit(id))} data-anchor={id}>
        <span className="sr-only">{label}</span>
        <span aria-hidden className="contents">
          {body}
        </span>
      </div>
    )
  return (
    <Button
      variant="ghost"
      className={cn('fb-hit fb-rowhit fb-team', lit(id))}
      data-node={id}
      data-anchor={id}
      aria-label={label}
      onClick={() => hit(id)}
    >
      {body}
    </Button>
  )
}

function Teams({ board, hit, lit }: { board: Board; hit: Hit; lit: Lit }) {
  const named = board.teams.filter((t) => t.id).length
  const max = Math.max(1, ...board.teams.map((t) => t.spent))
  return (
    <Card
      id="teams"
      title={copy.teams}
      className="fb-teams"
      count={<span className="fb-count">{copy.teamsMeta(named, board.people)}</span>}
    >
      {/* The org's own line starts here and branches to every team. */}
      <span className="fb-orgport" data-anchor={ORG} aria-hidden />
      <div className="fb-teamlist">
        {board.teams.map((t) => (
          <Team key={t.id ?? TEAMLESS} t={t} max={max} hit={hit} lit={lit} />
        ))}
      </div>
    </Card>
  )
}

function Agents({ board, hit, lit }: { board: Board; hit: Hit; lit: Lit }) {
  return (
    <Card
      id="agents"
      title={copy.agents}
      count={<Count shown={board.agents.length} all={board.agentCount} />}
    >
      <div className="fb-agents">
        {board.agents.map((a) => (
          <Button
            key={a.id}
            variant="ghost"
            className={cn('fb-hit fb-rowhit fb-agent', lit(a.id))}
            data-node={a.id}
            data-anchor={a.id}
            aria-label={`${a.name}, ${fmtMoney(a.spend)}, ${copy.tone[a.now]}`}
            onClick={() => hit(a.id)}
          >
            <span className="fb-name">{a.name}</span>
            <span className={cn('fb-val', a.costly && 'fb-red')}>{copy.usd(a.spend)}</span>
            <span className="fb-tiles" aria-hidden>
              <span className={`fb-big t-${a.now}`} title={tip(0, a.now)} />
              <span className="fb-days">
                {a.days.map((t, i) => (
                  // eslint-disable-next-line @eslint-react/no-array-index-key -- fixed day slots, oldest first
                  <span key={i} className={`t-${t}`} title={tip(a.days.length - 1 - i, t)} />
                ))}
              </span>
            </span>
          </Button>
        ))}
      </div>
    </Card>
  )
}

/** A workflow in the Teams card's row shape: its runs as a bar (against the busiest), its steps as tiles, its state. */
function Workflow({ w, max, hit, lit }: { w: WorkflowRow; max: number; hit: Hit; lit: Lit }) {
  const bad = w.steps.filter((s) => s.tone === 'bad').length
  const warn = w.steps.filter((s) => s.tone === 'warn').length
  const tone = w.failing ? 'bad' : warn ? 'warn' : 'ok'
  return (
    <Button
      variant="ghost"
      className={cn('fb-hit fb-rowhit fb-team fb-flow', lit(w.id))}
      data-node={w.id}
      data-anchor={w.id}
      aria-label={`${w.name}, ${copy.runs(w.runs)}, ${copy.flowState(bad, warn)}`}
      onClick={() => hit(w.id)}
    >
      <span className="fb-row">
        <span className="fb-name">{w.name}</span>
        <span className="fb-val">{copy.runs(w.runs)}</span>
      </span>
      <span className="fb-bbar" aria-hidden>
        <i className={cn('fb-bfill', `b-${tone}`)} style={{ width: pct(w.runs / max) }} />
      </span>
      <span className="fb-trow">
        <span className="fb-people" aria-hidden>
          {w.steps.map((s) => (
            <span
              key={s.key}
              className={`t-${s.tone}`}
              title={`${s.label}: ${copy.tone[s.tone]}`}
            />
          ))}
        </span>
        <span className={cn('fb-pace', bad ? 'fb-red' : warn && 'fb-warn')}>
          {copy.flowState(bad, warn)}
        </span>
      </span>
    </Button>
  )
}

function Workflows({ board, hit, lit }: { board: Board; hit: Hit; lit: Lit }) {
  const max = Math.max(1, ...board.workflows.map((w) => w.runs))
  return (
    <Card
      id="workflows"
      title={copy.workflows}
      count={<Count shown={board.workflows.length} all={board.workflowCount} />}
    >
      <div className="fb-teamlist fb-flowlist">
        {board.workflows.map((w) => (
          <Workflow key={w.id} w={w} max={max} hit={hit} lit={lit} />
        ))}
      </div>
    </Card>
  )
}

/** Every mark on the board is this size: providers, routers and tools alike. */
const MARK = 16

/**
 * A brand's mark on its tile. Providers keep their brand's colour (the providers are the only place the board shows
 * one); a tool or a router is a neutral tile with its mark.
 */
function Mark({
  brand,
  name,
  kind,
}: {
  brand: string | null
  name: string
  kind: 'tool' | 'provider'
}) {
  const key = brand === 'anthropic' ? 'claude' : brand === 'google' ? 'gemini' : brand
  const path = key ? BRAND_PATHS[key] : undefined
  const s: CSSProperties = { width: MARK, height: MARK }
  if (kind === 'provider' && key === 'gemini')
    return (
      <span className="fb-mark m-gemini" style={s} aria-hidden>
        <svg viewBox="0 0 24 24">
          <path d={path} />
        </svg>
      </span>
    )
  return (
    <span
      className={cn('fb-mark', kind === 'provider' ? `m-${key ?? 'none'}` : 'm-tool')}
      style={s}
      aria-hidden
    >
      {path ? (
        <svg viewBox="0 0 24 24">
          <path d={path} />
        </svg>
      ) : (
        <span className="fb-mono">{name.slice(0, 1).toUpperCase()}</span>
      )}
    </span>
  )
}

/** A state dot, drawn only where something needs a look (failing, slow or costly). */
function StateDot({ tone }: { tone: Tone }) {
  return tone === 'bad' || tone === 'warn' ? (
    <span className={cn('fb-dot', `t-${tone}`)} aria-hidden />
  ) : null
}

/** Connectors and tools: their marks only, most used first, a recently turned-off one faded. */
function Tools({
  rows,
  all,
  asOf,
  hit,
  lit,
}: {
  rows: ToolRow[]
  all: number
  asOf: number
  hit: Hit
  lit: Lit
}) {
  const t = copy.tool
  return (
    <Card id="tools" title={copy.tools} count={<Count shown={rows.length} all={all} />}>
      {rows.length ? (
        <div className="fb-toolgrid">
          {rows.map((r) => {
            const used = copy.usesTitle(r.agents, r.harnesses, r.steps)
            const off = r.offSince ? t.off((asOf - Date.parse(r.offSince)) / 86_400_000) : null
            const words = [
              r.name,
              copy.uses(uses(r)),
              off,
              r.tone === 'bad' || r.tone === 'warn' ? copy.tone[r.tone] : null,
            ]
            return (
              <Button
                key={r.id}
                variant="ghost"
                className={cn('fb-hit fb-rowhit fb-tool', off && 'fb-off', lit(r.id))}
                data-node={r.id}
                data-anchor={r.id}
                aria-label={words.filter(Boolean).join(', ')}
                title={`${r.name} · ${off ?? used}`}
                onClick={() => hit(r.id)}
              >
                <Mark brand={r.brand} name={r.name} kind="tool" />
                <StateDot tone={r.tone} />
              </Button>
            )
          })}
        </div>
      ) : (
        <p className="fb-empty">{t.empty}</p>
      )}
    </Card>
  )
}

/** Local harnesses, across the bottom: a block per kind, its installs by spend in seats, the free ones grey. */
function Harnesses({
  cols,
  all,
  hit,
  lit,
  dim,
}: {
  cols: HarnessCol[]
  all: number
  hit: Hit
  lit: Lit
  dim: Lit
}) {
  return (
    <Card id="harness" title={copy.harness} count={<Count shown={all} all={all} />}>
      <div className="fb-harness">
        {cols.map((c) => (
          <div
            key={c.key}
            className={cn('fb-hcol', dim([kindId(c.key), ...c.cells.map((x) => x.id)]))}
            data-anchor={kindId(c.key)}
          >
            <div className="fb-row">
              <span className="fb-name">{c.label}</span>
              <span
                className={cn('fb-val', c.failing && 'fb-red')}
                title={copy.installs(c.installs)}
              >
                {copy.usd0(c.spend)}
              </span>
            </div>
            <div className="fb-hgrid">
              {c.cells.map((x) => (
                <Button
                  key={x.id}
                  variant="ghost"
                  className={cn('fb-hit fb-cell', `h-${x.tone}`, lit(x.id))}
                  data-node={x.id}
                  aria-label={`${x.name}, ${x.spend === null ? copy.harnessTone.empty : fmtMoney(x.spend)}, ${copy.harnessTone[x.tone]}`}
                  title={`${x.name} · ${x.spend === null ? '—' : fmtMoney(x.spend)}`}
                  onClick={() => hit(x.id)}
                />
              ))}
              {Array.from({ length: c.slots }, (_, i) => (
                <span key={`slot-${i}`} className="fb-slot" title={copy.slots(c.slots)} />
              ))}
            </div>
          </div>
        ))}
      </div>
    </Card>
  )
}

/** LLM routers: a small tile each (its main provider's mark), a dot on one that is down; nothing else. */
function Routers({
  routers,
  all,
  hit,
  lit,
}: {
  routers: RouterCard[]
  all: number
  hit: Hit
  lit: Lit
}) {
  return (
    <Card
      id="routers"
      title={copy.routers}
      inline
      count={<Count shown={routers.length} all={all} />}
    >
      {routers.length ? (
        <div className="fb-chips">
          {routers.map((r) => {
            const p = r.providers[0]
            return (
              <Button
                key={r.id}
                variant="ghost"
                className={cn('fb-hit fb-rowhit fb-router', lit(r.id))}
                data-node={r.id}
                data-anchor={r.id}
                aria-label={copy.routerLabel(
                  copy.routerTile(r.name, r.tone === 'bad'),
                  fmtMoney(r.spend),
                  copy.share(r.share),
                )}
                title={copy.routerTile(r.name, r.tone === 'bad')}
                onClick={() => hit(r.id)}
              >
                <Mark brand={p?.brand ?? null} name={r.name} kind="tool" />
                <StateDot tone={r.tone} />
              </Button>
            )
          })}
        </div>
      ) : (
        <p className="fb-empty">{copy.noRoutedSpend}</p>
      )}
    </Card>
  )
}

/** Providers and models: a column per provider, a bar per model, every column on one scale. */
function Providers({
  providers,
  models,
  all,
  hit,
  lit,
  dim,
}: {
  providers: ProviderRow[]
  models: number
  all: number
  hit: Hit
  lit: Lit
  dim: Lit
}) {
  return (
    <Card
      id="providers"
      title={copy.providers}
      count={<span className="fb-count">{copy.providersMeta(all, models)}</span>}
    >
      <div
        className="fb-pcols"
        style={{ gridTemplateColumns: `repeat(${Math.max(1, providers.length)}, minmax(0, 1fr))` }}
      >
        {providers.map((p) => (
          <div key={p.id} className={cn('fb-pcol', dim([p.id, ...p.models.map((m) => m.id)]))}>
            <Button
              variant="ghost"
              className={cn('fb-hit fb-rowhit fb-phead', lit(p.id))}
              data-node={p.id}
              data-anchor={p.id}
              aria-label={`${p.name}, ${fmtMoney(p.spend)}`}
              title={`${p.name} · ${fmtMoney(p.spend)}${p.rest > 0 ? ` · ${copy.others} ${fmtMoney(p.rest)}` : ''}`}
              onClick={() => hit(p.id)}
            >
              <Mark brand={p.brand} name={p.name} kind="provider" />
              <span className="fb-name">{p.name}</span>
            </Button>
            {p.models.map((m, i) => (
              <Button
                key={m.id}
                variant="ghost"
                className={cn('fb-hit fb-rowhit fb-model', lit(m.id))}
                data-node={m.id}
                data-anchor={m.id}
                aria-label={`${m.name}, ${fmtMoney(m.spend)}, ${copy.share(m.share)}`}
                title={`${m.name} · ${fmtMoney(m.spend)} · ${copy.share(m.share)}`}
                onClick={() => hit(m.id)}
              >
                <span className="fb-mrow">
                  <span className="fb-name">{m.name}</span>
                  <span className="fb-val">{copy.usd0(m.spend)}</span>
                </span>
                <span className="fb-bar">
                  <i
                    className={cn(`p-${p.spend > 0 ? (p.brand ?? 'none') : 'none'}`, `s${i}`)}
                    style={{ width: pct(m.bar) }}
                  />
                </span>
              </Button>
            ))}
          </div>
        ))}
      </div>
    </Card>
  )
}

const SPARK_W = 276
const SPARK_H = 132

/**
 * The daily spend line: faded where it starts, the budget pace (or mean) behind it, the peaks over it as dots. Each
 * dot is a button over the line that picks its day.
 */
function Spark({
  series,
  threshold,
  spikes,
  dates,
  day,
  onDay,
}: SpendCard & { dates: readonly string[]; day: number | null; onDay(i: number): void }) {
  const g = useId()
  if (series.length < 2) return null
  const [w, h] = [SPARK_W, SPARK_H]
  const max = Math.max(...series, threshold ?? 0) * 1.05 || 1
  const min = Math.min(...series, threshold ?? Infinity) * 0.9
  const x = (i: number) => (i / (series.length - 1)) * (w - 12) + 6
  const y = (val: number) => h - 8 - ((val - min) / (max - min || 1)) * (h - 16)
  const d = series
    .map((val, i) => `${i ? 'L' : 'M'}${x(i).toFixed(1)},${y(val).toFixed(1)}`)
    .join('')
  return (
    <div className="fb-sparkbox" style={{ width: w, height: h }} data-keep="">
      <svg className="fb-spark" viewBox={`0 0 ${w} ${h}`} width={w} height={h} aria-hidden>
        <defs>
          <linearGradient id={g} x1="0" x2={w} y1="0" y2="0" gradientUnits="userSpaceOnUse">
            <stop offset="0" stopColor="var(--fb-spark)" stopOpacity="0.15" />
            <stop offset="0.45" stopColor="var(--fb-spark)" stopOpacity="0.75" />
            <stop offset="1" stopColor="var(--fb-spark)" stopOpacity="1" />
          </linearGradient>
        </defs>
        {threshold !== null ? (
          <line x1={6} x2={w - 6} y1={y(threshold)} y2={y(threshold)} className="fb-thresh" />
        ) : null}
        <path d={d} fill="none" stroke={`url(#${g})`} strokeWidth={2} strokeLinejoin="round" />
      </svg>
      {spikes.map((i) => (
        <Button
          key={i}
          variant="ghost"
          className={cn('fb-hit fb-spike', day === i && 'fb-sel')}
          style={{ left: x(i), top: y(series[i] ?? 0) }}
          aria-label={copy.dayLabel(fmtShortDay(dates[i] ?? ''), fmtMoney(series[i] ?? 0))}
          aria-pressed={day === i}
          title={`${fmtShortDay(dates[i] ?? '')} · ${fmtMoney(series[i] ?? 0)}`}
          onClick={() => onDay(i)}
        />
      ))}
    </div>
  )
}

/** A bill line: the name, a dotted leader and the amount. */
function BillRow({
  name,
  amount,
  className,
  title,
  lead,
}: {
  name: string
  amount: string
  className?: string
  title?: string
  lead?: ReactNode
}) {
  return (
    <span className={cn('fb-bline', className)} title={title}>
      {lead}
      <span className="fb-name">{name}</span>
      <span className="fb-leader" aria-hidden />
      <span className="fb-val">{amount}</span>
    </span>
  )
}

/**
 * Total spend as a bill: the range's spend added up by provider, then by source, then what optimizations took off it,
 * each side coming to the same total, and the daily line under it. A provider's line picks the provider; hovering the
 * bill turns every line it pays for green.
 */
function BillCard({
  bill,
  card,
  dates,
  day,
  saved,
  hit,
  lit,
  onDay,
  onPaying,
}: {
  bill: Bill
  card: SpendCard
  dates: readonly string[]
  day: number | null
  saved: number | null
  hit: Hit
  lit: Lit
  onDay(i: number): void
  onPaying(on: boolean): void
}) {
  const b = copy.bill
  const up = (card.changePct ?? 0) > 0
  return (
    <Card
      id="bill"
      title={copy.totalSpend}
      className="fb-billcard"
      anchor={BILL}
      hover={onPaying}
      count={
        card.changePct !== null ? (
          <span
            className={cn('fb-change', up && 'up')}
            aria-label={copy.changeLabel(card.changePct)}
          >
            <span aria-hidden>{up ? '↑' : '↓'}</span> {copy.change(card.changePct)}
          </span>
        ) : null
      }
    >
      <span className="fb-amount">{fmtMoney(bill.total)}</span>
      <p className="fb-sub">{b.period(Math.max(1, card.series.length))}</p>
      <span className="fb-rule" aria-hidden />
      <p className="fb-bhead">{b.byProvider}</p>
      {bill.providers.map((p) => (
        <Button
          key={p.id}
          variant="ghost"
          className={cn('fb-hit fb-rowhit fb-billrow', lit(p.id))}
          data-node={p.id}
          aria-label={b.line(p.name, fmtMoney(p.spend))}
          onClick={() => hit(p.id)}
        >
          <BillRow
            name={p.name}
            amount={fmtMoney(p.spend)}
            lead={<Mark brand={p.brand} name={p.name} kind="provider" />}
          />
        </Button>
      ))}
      {bill.otherProviders > 0 ? (
        <BillRow name={b.otherProviders} amount={fmtMoney(bill.otherProviders)} />
      ) : null}
      {bill.unattributed > 0 ? (
        <BillRow
          name={b.unattributed}
          amount={fmtMoney(bill.unattributed)}
          title={b.unattributedTitle}
        />
      ) : null}
      <span className="fb-rule" aria-hidden />
      <p className="fb-bhead">{b.bySource}</p>
      <BillRow name={b.agents} amount={fmtMoney(bill.agents)} />
      <BillRow name={b.harnesses} amount={fmtMoney(bill.harnesses)} />
      <span className="fb-rule" aria-hidden />
      {saved !== null && saved > 0 ? (
        <>
          <BillRow name={b.before} amount={fmtMoney(bill.total + saved)} />
          <BillRow name={b.saved} amount={`−${fmtMoney(saved)}`} className="fb-saveline" />
        </>
      ) : null}
      <BillRow name={b.total} amount={fmtMoney(bill.total)} className="fb-btotal" />
      <p className="fb-bhead fb-perday">{b.perDay}</p>
      <Spark {...card} dates={dates} day={day} onDay={onDay} />
    </Card>
  )
}

/** A point on the top half-circle: t = 0 at the left end, 1 at the right. */
const onArc = (cx: number, cy: number, r: number, t: number) => {
  const a = Math.PI * (1 - Math.min(1, Math.max(0, t)))
  return [cx + r * Math.cos(a), cy - r * Math.sin(a)] as const
}
const arc = (cx: number, cy: number, r: number, t0: number, t1: number) => {
  const [x0, y0] = onArc(cx, cy, r, t0)
  const [x1, y1] = onArc(cx, cy, r, t1)
  return `M${x0.toFixed(2)},${y0.toFixed(2)}A${r},${r} 0 0 1 ${x1.toFixed(2)},${y1.toFixed(2)}`
}

/** The month's budget used, on a half-circle in the teams' tones, with the alert's tick. */
function Gauge({ used, tone }: { used: number; tone: Tone }) {
  const [cx, cy, r] = [60, 62, 48]
  const [t0x, t0y] = onArc(cx, cy, r - 10, ALERT)
  const [t1x, t1y] = onArc(cx, cy, r + 10, ALERT)
  return (
    <svg className="fb-gauge" viewBox="0 0 120 70" aria-hidden>
      <path d={arc(cx, cy, r, 0, 1)} className="fb-gtrack" strokeWidth={12} fill="none" />
      {used > 0 ? (
        <path
          d={arc(cx, cy, r, 0, Math.min(1, used))}
          className={cn('fb-garc', `g-${tone}`)}
          strokeWidth={12}
          fill="none"
        />
      ) : null}
      <line x1={t0x} y1={t0y} x2={t1x} y2={t1y} className="fb-gtick" />
    </svg>
  )
}

/** Budgeting: the month's gauge (it opens the token battery) over what no budget governs. */
function Budget({
  hud,
  mine,
  selected,
  lit,
  onOpen,
}: {
  hud: Hud
  mine: boolean
  selected: boolean
  lit: Lit
  onOpen(): void
}) {
  const b = hud.budget
  const title = mine ? copy.allowance : copy.budget
  const used = b && b.limit > 0 ? b.spent / b.limit : 0
  const tone: Tone = !b ? 'idle' : b.pace > 1 || used >= 1 ? 'bad' : used >= ALERT ? 'warn' : 'ok'
  return (
    <Card
      id="budget"
      title={title}
      className="fb-budget"
      anchor={BUDGET}
      count={
        b ? (
          <span className={cn('fb-chip', b.pace > 1 && 'up')}>{copy.budgetPace(b.pace)}</span>
        ) : null
      }
    >
      {b ? (
        <Button
          variant="ghost"
          className={cn('fb-hit fb-rowhit fb-bread', lit(ORG))}
          data-node={ORG}
          aria-label={`${title}: ${copy.budgetLabel(b.spent, b.limit, b.pace)}`}
          aria-pressed={selected}
          onClick={onOpen}
        >
          <Gauge used={used} tone={tone} />
          <span className="fb-breadout">
            <span className="fb-bamount">{copy.usd0(b.spent)}</span>
            <span className="fb-sub">
              {copy.budgetOf(b.limit)} · {copy.share(used)}
            </span>
            <span className={cn('fb-sub', b.pace > 1 && 'fb-red')}>
              {copy.projected(b.limit * b.pace)}
            </span>
          </span>
        </Button>
      ) : (
        <p className="fb-sub">{copy.noBudget}</p>
      )}
      {mine ? null : (
        <div className="fb-bunder" title={copy.ungovernedTitle}>
          <span className="fb-bline fb-btotal">
            <span className="fb-name">{copy.ungoverned}</span>
            <span className="fb-leader" aria-hidden />
            <span className={cn('fb-val', hud.ungoverned > 0 && 'fb-red')}>
              {copy.usd0(hud.ungoverned)}
            </span>
          </span>
          <BillRow name={copy.noBudgetLine} amount={copy.usd0(hud.unbudgeted)} />
          <BillRow
            name={copy.alertOnly}
            amount={copy.usd0(hud.uncapped)}
            title={copy.alertOnlyTitle}
          />
        </div>
      )}
    </Card>
  )
}

/** What was saved: a slowly turning globe in the teams' greens (still under reduced motion). A click picks it. */
function Saved({ saved, lit }: { saved: number | null; lit: string | undefined }) {
  const { card, onCard } = use(Pick)
  const amount = fmtMoney(saved ?? 0)
  const r = 92
  const on = card === 'saved'
  return (
    <Button
      variant="ghost"
      className={cn('fb-hit fb-saved', lit, on && 'fb-cardsel', card && !on && 'fb-cardfade')}
      style={CARDS.saved}
      data-card="saved"
      aria-label={copy.savedLabel(amount)}
      aria-pressed={on}
      onClick={() => onCard('saved')}
    >
      <svg className="fb-globe" viewBox="-100 -100 200 200" aria-hidden>
        <circle r={r} className="fb-gshell" />
        {[-60, -30, 0, 30, 60].map((lat) => (
          <ellipse
            key={`lat${lat}`}
            cy={Math.sin((lat * Math.PI) / 180) * r}
            rx={Math.cos((lat * Math.PI) / 180) * r}
            ry={Math.cos((lat * Math.PI) / 180) * r * 0.16}
            className="fb-glat"
          />
        ))}
        {[0, 1, 2, 3, 4, 5].map((k) => (
          <ellipse
            key={`lon${k}`}
            rx={r}
            ry={r}
            className="fb-glon"
            style={{ animationDelay: `${-k * 2}s` }}
          />
        ))}
      </svg>
      <span className="fb-savedval">{amount}</span>
      <span className="fb-savedlabel">{copy.saved}</span>
      <span className="fb-savedsub">{copy.savedSub}</span>
    </Button>
  )
}

/** Rows the Opportunities card shows; the drawer lists them all. */
const OPPS_SHOWN = 3

/** Opportunities: the ways to save more, largest first, each a pick that the drawer explains. */
function Opportunities({
  v,
  sel,
  onPick,
  lit,
}: {
  v: EstateView
  sel: string | null
  onPick(id: string): void
  lit: string | undefined
}) {
  const t = copy.opps
  const opps = visibleOpps(v)
  const total = opps.reduce((n, o) => n + o.saving_usd, 0)
  return (
    <Card
      id="opportunities"
      title={t.title}
      className={cn('fb-oppcard', lit)}
      count={opps.length ? <span className="fb-count">{t.meta(opps.length)}</span> : null}
    >
      {opps.length ? (
        <div className="fb-opps">
          {opps.slice(0, OPPS_SHOWN).map((o) => (
            <Button
              key={o.id}
              variant="ghost"
              className={cn('fb-hit fb-rowhit fb-opp', sel === o.id && 'fb-sel')}
              aria-label={t.label(o.title, fmtMoney(o.saving_usd))}
              aria-pressed={sel === o.id}
              onClick={() => onPick(o.id)}
            >
              <span className="fb-name">{o.title}</span>
              <span className="fb-val fb-good">{t.month(copy.usd0(o.saving_usd))}</span>
            </Button>
          ))}
          <p className="fb-sub fb-opptotal">{t.total(copy.usd0(total))}</p>
        </div>
      ) : (
        <p className="fb-empty">{t.none}</p>
      )}
    </Card>
  )
}
