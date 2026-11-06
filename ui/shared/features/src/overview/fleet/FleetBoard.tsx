/**
 * The Overview's Map: the "Fleet Runtime / TokenOps" board (the user's design, 2026-10-16), drawn inside the Map
 * canvas (`map/MapCanvas.tsx`: the HUD, the drawer and pan and zoom), laid out as five columns the spend flows through
 * (Ans's wireframe, 2026-11-04): Org (every team against its budget, its people as tiles); the agents, the local
 * harnesses and the workflows, stacked; the LLM routers (a row each, then No router) over the connectors and tools; the
 * providers (a section each, its models' bars on one scale); and TokenOps: the bill (the range's spend added up by
 * provider and by source, to one total), what was saved and the month's budget. Token lines join the parts (lines.ts):
 * one smooth curve per connection across the gutter between two columns, drawn under the cards. A line shows only for
 * what is hovered, focused or picked (Ans, 2026-11-02: the board at rest is just its cards); hovering the bill shows
 * every line it pays for.
 *
 * The board is HTML at its own coordinates (layout.ts); the canvas's viewport sets the lines' SVG viewBox and the
 * board's transform together, so the two always line up. Picking a part puts `node:<id>` in the URL: it fills like the
 * sidebar's current page, what it touches stays lit with its lines, the rest dims, and the canvas's drawer explains it.
 * A click on a card anywhere but a part (or on its title) picks the card (`card:<id>`): a light ring on it, the other
 * cards faded, its lines lit, and its insights in the drawer, whose buttons go on to the pages (cards.ts). Nothing on
 * the board navigates by itself. A harness kind's row picks the whole kind (`kind:<harness>`), like a part. Hovering
 * the bill turns every line it pays for green; red stays where spend is going up.
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
import { Link } from '@tanstack/react-router'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import { fmtMoney } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import { copy as overviewCopy } from '../copy'
import { estateCopy } from '../estate/copy'
import { STEP_TO, type StepId, type StepTo } from '../estate/setup'
import type { EstateView } from '../estate/useEstateView'
import { BRAND_PATHS } from '../map/brands'
import type { ViewState } from '../map/useViewport'
import type { Hud } from '../village/model'
import { boardSel, cardOf, cardParts } from './cards'
import { boardCopy as copy } from './copy'
import { BOARD, CARDS, GAP, SECTIONS, TITLE_TOP, type CardId } from './layout'
import {
  billLine,
  buildLines,
  curve,
  focusOf,
  kindFocus,
  kindId,
  litLine,
  ORG,
  pointsOf,
  restLines,
  TEAMLESS,
  type Anchor,
  type Line,
} from './lines'
import {
  type AgentRow,
  type Bill,
  type Board,
  type DirectRow,
  type HarnessCol,
  type ProviderRow,
  type RouterCard,
  type SpendCard,
  type TeamRow,
  type Tone,
  type ToolRow,
  type WorkflowRow,
  AGENT_DAYS,
  WORKFLOW_DAYS,
  uses,
} from './model'
import './board.css'

/** The month's budget reads orange once it passes this share of the limit (its tick on the gauge). */
const ALERT = 0.8

interface Props {
  v: EstateView
  board: Board
  /** The bill's change on the window before (model.ts `billSeries`). */
  card: SpendCard
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
  const own = nodeId && v.nodes.has(nodeId) ? nodeId : null
  const oppId = sel?.startsWith('opp:') ? sel.slice('opp:'.length) : null
  const opp = oppId ? e.opportunities.find((o) => o.id === oppId) : undefined
  // A problem or a way to save (the Control center's Take me there) picks the part it is about, so its connections
  // show as if that part were clicked (Ans, 2026-11-05); the drawer still explains the problem or the saving.
  const probId = sel?.startsWith('problem:') ? sel.slice('problem:'.length) : null
  const about = (probId ? v.problems.find((p) => p.id === probId)?.entities : opp?.entities)?.find(
    (x) => v.nodes.has(x),
  )
  const picked = boardSel(sel)
  const pickedCard = cardOf(picked)
  const node = own ?? about ?? null
  const kind = picked?.type === 'kind' ? picked.key : null
  const selKey = node ?? (kind ? kindId(kind) : null)
  const focus = useMemo(() => {
    if (kind) return kindFocus(v, kind)
    if (!node) return opp ? new Set(opp.entities) : null
    const f = focusOf(v, node)
    opp?.entities.forEach((x) => f.add(x))
    return f
  }, [v, node, kind, opp])
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
  // The part under the pointer (or keyboard focus): its lines show as a pick's would. A pick wins over it.
  const [hovered, setHovered] = useState<string | null>(null)
  const hover = !sel && hovered && v.nodes.has(hovered) ? hovered : null
  const hoverFocus = useMemo(() => (hover ? focusOf(v, hover) : null), [v, hover])

  const outer = useRef<HTMLDivElement>(null)
  const root = useRef<HTMLDivElement>(null)
  // The board and its overlay follow the viewport: the same view the lines' viewBox shows.
  useLayoutEffect(() => {
    const els = [root.current]
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
  /** A harness kind picked again clears it. */
  const onKind = useMemo(
    () => (key: string) => (sel === `kind:${key}` ? onClear() : onSelect(`kind:${key}`)),
    [sel, onClear, onSelect],
  )
  // A click on a card (not on a part or a button) picks it; anywhere else off the cards clears.
  useLayoutEffect(() => {
    const el = outer.current
    if (!el) return
    const click = (ev: MouseEvent) => {
      const t = ev.target as Element
      if (t.closest('[data-node],a,button,[data-keep]')) return
      // Anywhere on a harness kind's row picks the whole kind.
      const k = t.closest<HTMLElement>('[data-kind]')?.dataset.kind
      if (k) return onKind(k)
      const id = t.closest<HTMLElement>('[data-card]')?.dataset.card
      if (id && id in CARDS) onCard(id as CardId)
      else onBackground()
    }
    el.addEventListener('click', click)
    return () => el.removeEventListener('click', click)
  }, [onCard, onKind, onBackground])

  useLayoutEffect(() => {
    const el = outer.current
    if (!el) return
    const enter = (ev: Event) =>
      setHovered((ev.target as Element).closest<HTMLElement>('[data-node]')?.dataset.node ?? null)
    const leave = () => setHovered(null)
    el.addEventListener('pointerover', enter)
    el.addEventListener('pointerleave', leave)
    el.addEventListener('focusin', enter)
    el.addEventListener('focusout', leave)
    return () => {
      el.removeEventListener('pointerover', enter)
      el.removeEventListener('pointerleave', leave)
      el.removeEventListener('focusin', enter)
      el.removeEventListener('focusout', leave)
    }
  }, [])

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
  const lineProps = {
    lines,
    at,
    selKey: selKey ?? hover,
    focus: focus ?? hoverFocus,
    parts,
    running,
    all: paying,
  }

  return (
    <div ref={outer} className={cn('fb', paying && 'fb-paying')} data-testid="overview-canvas">
      <Lines {...lineProps} />
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
          <Stack at="teams">
            <Teams board={board} hit={hit} lit={lit} />
          </Stack>
          {/* Local harnesses above the workflows (Ans, 2026-11-04). */}
          <Stack at="agents">
            <Agents board={board} hit={hit} lit={lit} />
            <Harnesses
              cols={board.harnesses}
              all={board.harnessCount}
              selKey={selKey}
              hit={hit}
              onKind={onKind}
              lit={lit}
              dim={dim}
            />
            <Workflows board={board} hit={hit} lit={lit} />
          </Stack>
          <Stack at="routers">
            <Routers
              routers={board.routers}
              direct={board.direct}
              all={board.routerCount}
              hit={hit}
              lit={lit}
              dim={dim}
              node={(id) => v.nodes.has(id)}
            />
            <Tools
              rows={board.tools}
              all={board.toolCount}
              asOf={Date.parse(e.as_of)}
              hit={hit}
              lit={lit}
            />
          </Stack>
          <Stack at="providers">
            <Providers
              providers={board.providers}
              models={e.models.length}
              all={e.providers.length}
              hit={hit}
              lit={lit}
              dim={dim}
            />
          </Stack>
          <Stack at="bill">
            <BillCard bill={board.bill} card={card} hit={hit} lit={lit} onPaying={setPaying} />
            <Saved total={board.bill.total} saved={saved} lit={savingsLit} />
            <Budget
              hud={hud}
              mine={mine}
              selected={pickedCard === 'budget'}
              lit={lit}
              onOpen={() => (pickedCard === 'budget' ? onClear() : onSelect('card:budget'))}
            />
          </Stack>
        </div>
      </Pick>
    </div>
  )
}

/**
 * The token lines, under the cards: only the ones the pick or hover lights (every one while `all`), unlit first, so
 * what a selection lights draws on top. Calls light only where they touch the pick itself (or the picked tools or
 * workflows), so picking a team shows where its money goes without every tool its agents call.
 */
function Lines({
  lines,
  at,
  selKey,
  focus,
  parts,
  running,
  all,
}: {
  lines: Line[]
  at: ReadonlyMap<string, Anchor>
  selKey: string | null
  focus: ReadonlySet<string> | null
  /** A picked card's parts: their lines are lit, the rest are not. */
  parts: ReadonlySet<string> | null
  running: boolean
  /** Draw every line, not just the lit ones (hovering the bill). */
  all: boolean
}) {
  // Nothing picked or hovered: only what is breaking or getting dearer, simplified (`restLines`).
  const rest = !all && !parts && !focus
  const drawn = useMemo(() => {
    const max = Math.max(1, ...lines.map((l) => l.usd))
    return (rest ? restLines(lines) : lines).flatMap((l) => {
      const pts = pointsOf(l, at)
      if (!pts || pts.length < 2) return []
      const w = l.kind === 'spend' ? 1.2 + 3.3 * Math.sqrt(l.usd / max) : 1
      return [{ l, d: curve(pts), w }]
    })
  }, [lines, at, rest])
  const on = (l: Line) => {
    if (l.kind === 'calls' && (parts || selKey)) {
      if (parts) return parts.has(l.to) || (l.route.startsWith('flow') && parts.has(l.from))
      return l.from === selKey || l.to === selKey
    }
    return parts
      ? parts.has(l.from) || parts.has(l.to)
      : focus && selKey
        ? litLine(l, selKey, focus)
        : focus
          ? focus.has(l.from) || focus.has(l.to)
          : null
  }
  // Lit over unlit, failing over red over blue (so a shared stretch shows the worst), spend over calls.
  const rank = (l: Line) =>
    Number(on(l) === true) * 8 +
    Number(l.failing) * 4 +
    Number(l.hot) * 2 +
    Number(l.kind === 'spend')
  const order = drawn
    .filter(({ l }) => rest || all || on(l) === true)
    .sort((a, b) => rank(a.l) - rank(b.l))
  return (
    // The zone's first svg: the viewport writes its viewBox.
    <svg className="fb-lines" aria-hidden>
      {order.map(({ l, d, w }) => {
        const o = on(l)
        const cls = cn(
          l.kind,
          l.hot && 'hot',
          l.failing && 'failing',
          o === true && 'on',
          o === false && 'off',
        )
        return (
          <g key={l.id} data-line={l.id}>
            <path d={d} className={cn('fb-ln', cls)} strokeWidth={w} />
            {running && l.kind === 'spend' && o !== false ? (
              <path d={d} className={cn('fb-run', cls)} strokeWidth={w + 1} />
            ) : null}
          </g>
        )
      })}
    </svg>
  )
}

/** What the lines mean: the Map canvas hangs it from its top right corner (Ans, 2026-11-04). */
export function BoardLegend() {
  const items = [
    ['spend', copy.legend.spend],
    ['spend hot', copy.legend.hot],
    ['spend failing', copy.legend.failing],
    ['calls', copy.legend.calls],
  ] as const
  return (
    // `fb-tones`: the legend hangs on the canvas, outside the board, so it takes the palette itself.
    <p className="fb-legend fb-tones">
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

/** A column: its cards stacked from its first card's place, each as tall as it holds, GAP apart. */
function Stack({ at, children }: { at: CardId; children: ReactNode }) {
  const b = CARDS[at]
  return (
    <div className="fb-stack" style={{ left: b.left, top: b.top, width: b.width, gap: GAP }}>
      {children}
    </div>
  )
}

/**
 * A card: its title (a button that picks the card) and count over its body. Picked, it carries a light ring and the
 * other cards fade. Its box is anchor `card:<id>`: where lines land on the card itself (the bill, the budget), and
 * where the canvas pans to.
 */
function Card({
  id,
  title,
  count,
  children,
  className,
  hover,
}: {
  id: CardId
  title: string
  count?: ReactNode
  children: ReactNode
  className?: string
  hover?: (on: boolean) => void
}) {
  const h = useId()
  const { card, onCard } = use(Pick)
  const on = card === id
  return (
    <section
      className={cn('fb-card', on && 'fb-cardsel', card && !on && 'fb-cardfade', className)}
      aria-labelledby={h}
      data-card={id}
      data-anchor={`card:${id}`}
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

/** Where an empty card's step goes: one of the canvases' setup steps, or a page of its own. */
interface Go {
  label: string
  to: StepTo | '/deploy' | '/optimization'
  search?: { tab: 'providers' }
}
const stepGo = (id: StepId): Go => ({ label: overviewCopy.needs.setup[id].title, to: STEP_TO[id] })
/** The ghost rows' name widths, in percent: a short, uneven list rather than a ruler. */
const GHOST = [62, 46, 34]

/**
 * An empty card's body: faint rows in the card's own shape (with day tiles under them for the cards that draw days),
 * fading out, then what lands here once it fills and the one step that fills it. The step is a link, the board's only
 * one: an empty card has nothing to pick yet.
 */
function Blank({
  text,
  go,
  rows = 3,
  tiles,
}: {
  text: string
  go?: Go
  rows?: number
  tiles?: boolean
}) {
  return (
    <div className="fb-blank">
      <span className="fb-ghost" aria-hidden>
        {GHOST.slice(0, rows).map((w) => (
          <span key={w} className="fb-grow">
            <span className="fb-gline">
              <i className="fb-gmark" />
              <i style={{ width: `${w}%` }} />
              <i className="fb-gval" />
            </span>
            {tiles ? <i className="fb-gtiles" /> : null}
          </span>
        ))}
      </span>
      <p className="fb-btext">{text}</p>
      {go ? (
        <Link
          to={go.to}
          search={(go.search ?? {}) as never}
          {...asButton({ variant: 'outline', size: 'xs', className: 'fb-bgo' })}
        >
          {go.label}
        </Link>
      ) : null}
    </div>
  )
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
      {board.teams.length ? (
        <div className="fb-teamlist">
          {board.teams.map((t) => (
            <Team key={t.id ?? TEAMLESS} t={t} max={max} hit={hit} lit={lit} />
          ))}
        </div>
      ) : (
        <Blank text={copy.empty.teams} rows={2} tiles />
      )}
    </Card>
  )
}

/** An agent's dot beside its name: red failing today, orange slow or costly, none when it's fine. */
const agentTone = (a: AgentRow): Tone =>
  a.now === 'bad' ? 'bad' : a.now === 'warn' || a.costly ? 'warn' : 'ok'

/** Agents: a row each, its dot where it needs a look, its spend, and its last 18 days as tiles, oldest first. */
function Agents({ board, hit, lit }: { board: Board; hit: Hit; lit: Lit }) {
  return (
    <Card
      id="agents"
      title={copy.agents}
      count={<Count shown={board.agents.length} all={board.agentCount} />}
    >
      {board.agents.length ? null : (
        <Blank
          text={copy.empty.agents}
          go={{ label: copy.empty.deploy, to: '/deploy' }}
          rows={2}
          tiles
        />
      )}
      <div className="fb-agents">
        {board.agents.map((a) => {
          const tone = agentTone(a)
          return (
            <Button
              key={a.id}
              variant="ghost"
              className={cn('fb-hit fb-rowhit fb-agent', lit(a.id))}
              data-node={a.id}
              data-anchor={a.id}
              aria-label={`${a.name}, ${fmtMoney(a.spend)}, ${copy.tone[a.now]}`}
              onClick={() => hit(a.id)}
            >
              <span className="fb-aname">
                <span className="fb-name">{a.name}</span>
                <StateDot tone={tone} />
              </span>
              <span className="fb-val">{copy.usd(a.spend)}</span>
              <DayTiles days={a.days} slots={AGENT_DAYS} className="fb-days" />
            </Button>
          )
        })}
      </div>
    </Card>
  )
}

/** Day tiles from the left, oldest first, then the empty slots a short window leaves (Ans, 2026-11-05). */
function DayTiles({ days, slots, className }: { days: Tone[]; slots: number; className: string }) {
  return (
    <span className={className} aria-hidden>
      {days.map((t, i) => (
        // eslint-disable-next-line @eslint-react/no-array-index-key -- fixed day slots, oldest first
        <span key={i} className={`t-${t}`} title={tip(days.length - 1 - i, t)} />
      ))}
      {Array.from({ length: Math.max(0, slots - days.length) }, (_, i) => (
        <span key={`e${i}`} className="t-none" />
      ))}
    </span>
  )
}

/**
 * A workflow: its name and runs, its steps as one bar cut into a segment each (red or orange where that step fails or
 * slows: Ans, 2026-11-05), its last WORKFLOW_DAYS days as tiles and its state.
 */
function Workflow({ w, hit, lit }: { w: WorkflowRow; hit: Hit; lit: Lit }) {
  const bad = w.steps.filter((s) => s.tone === 'bad').length
  const warn = w.steps.filter((s) => s.tone === 'warn').length
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
      <span className="fb-steps" aria-hidden>
        {w.steps.map((s, i) => (
          <i
            key={s.key}
            className={`b-${s.tone}`}
            title={copy.stepTitle(i + 1, s.label, copy.tone[s.tone] ?? s.tone)}
          />
        ))}
      </span>
      <span className="fb-trow">
        <DayTiles days={w.days} slots={WORKFLOW_DAYS} className="fb-people" />
        <span className={cn('fb-pace', bad ? 'fb-red' : warn && 'fb-warn')}>
          {copy.flowState(bad, warn)}
        </span>
      </span>
    </Button>
  )
}

function Workflows({ board, hit, lit }: { board: Board; hit: Hit; lit: Lit }) {
  return (
    <Card
      id="workflows"
      title={copy.workflows}
      count={<Count shown={board.workflows.length} all={board.workflowCount} />}
    >
      {board.workflows.length ? (
        <div className="fb-teamlist fb-flowlist">
          {board.workflows.map((w) => (
            <Workflow key={w.id} w={w} hit={hit} lit={lit} />
          ))}
        </div>
      ) : (
        <Blank text={copy.empty.workflows} go={stepGo('workflows')} rows={2} tiles />
      )}
    </Card>
  )
}

/** Every mark on the board is this size: providers, routers and tools alike. */
const MARK = 16
/**
 * A brand's mark on its tile. Providers keep their brand's colour (the providers are the only place the board shows
 * one); a tool is a neutral grey tile with its mark, and a router or a tool with no mark one with its initial (Ans,
 * 2026-11-05: brand colours and gradients took too much attention; a router borrows no provider's logo).
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

/** Connectors and tools: a row each (its mark, its name, a dot where it needs a look, what calls it), most used first. */
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
        <div className="fb-tlist">
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
                title={off ?? used}
                onClick={() => hit(r.id)}
              >
                <Mark brand={r.brand} name={r.name} kind="tool" />
                <span className="fb-name">{r.name}</span>
                <StateDot tone={r.tone} />
                <span className="fb-val">{uses(r)}</span>
              </Button>
            )
          })}
        </div>
      ) : (
        <Blank text={copy.empty.tools} go={stepGo('mcp')} />
      )}
    </Card>
  )
}

/** Local harnesses, under the agents: a row per kind, its installs by spend in seats across the card, the free ones grey. */
function Harnesses({
  cols,
  all,
  selKey,
  hit,
  onKind,
  lit,
  dim,
}: {
  cols: HarnessCol[]
  all: number
  selKey: string | null
  hit: Hit
  /** Picks a whole kind: its name row, or anywhere on its row but a seat. */
  onKind(key: string): void
  lit: Lit
  dim: Lit
}) {
  return (
    <Card id="harness" title={copy.harness} count={<Count shown={all} all={all} />}>
      {cols.length ? null : (
        <Blank text={copy.empty.harness} go={stepGo('harness')} rows={2} tiles />
      )}
      <div className="fb-harness">
        {cols.map((c) => (
          <div
            key={c.key}
            className={cn(
              'fb-hcol',
              selKey === kindId(c.key)
                ? 'fb-sel'
                : dim([kindId(c.key), ...c.cells.map((x) => x.id)]),
            )}
            data-anchor={kindId(c.key)}
            data-kind={c.key}
          >
            <Button
              variant="ghost"
              className="fb-hit fb-row fb-khead"
              aria-label={copy.kindLabel(c.label, fmtMoney(c.spend), c.installs)}
              aria-pressed={selKey === kindId(c.key)}
              onClick={() => onKind(c.key)}
            >
              <span className="fb-name">{c.label}</span>
              <span
                className={cn('fb-val', c.failing && 'fb-red')}
                title={copy.installs(c.installs)}
              >
                {copy.usd0(c.spend)}
              </span>
            </Button>
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

/** LLM routers: a row each (its main provider's mark, a dot on one that is down), its share of the agents' spend as a bar. */
function Routers({
  routers,
  direct,
  all,
  hit,
  lit,
  dim,
  node,
}: {
  routers: RouterCard[]
  direct: DirectRow | null
  all: number
  hit: Hit
  lit: Lit
  dim: Lit
  /** The No router row is a part you can pick only when the estate has its direct path. */
  node(id: string): boolean
}) {
  return (
    <Card id="routers" title={copy.routers} count={<Count shown={routers.length} all={all} />}>
      {routers.length ? (
        <div className="fb-rlist">
          {routers.map((r) => {
            return (
              <Button
                key={r.id}
                variant="ghost"
                className={cn('fb-hit fb-rowhit fb-rrow', lit(r.id))}
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
                <span className="fb-row">
                  <Mark brand={null} name={r.name} kind="tool" />
                  <span className="fb-name">{r.name}</span>
                  <StateDot tone={r.tone} />
                  <span className="fb-val">{copy.usd0(r.spend)}</span>
                </span>
                <span className="fb-bar" aria-hidden>
                  <i className="fb-rshare" style={{ width: pct(r.share) }} />
                </span>
              </Button>
            )
          })}
        </div>
      ) : null}
      {direct ? (
        <>
          {routers.length ? <span className="fb-rule" aria-hidden /> : null}
          <Direct d={direct} hit={node(direct.id) ? hit : null} lit={lit} dim={dim} />
        </>
      ) : null}
      {routers.length ? null : (
        <Blank text={copy.empty.routers} go={stepGo('router')} rows={direct ? 1 : 2} />
      )}
    </Card>
  )
}

/** The No router section: what reaches a model with no router between, its lines passing through it like a router's. */
function Direct({ d, hit, lit, dim }: { d: DirectRow; hit: Hit | null; lit: Lit; dim: Lit }) {
  const label = copy.direct.label(fmtMoney(d.spend), copy.share(d.share), d.agents, d.harnesses)
  const body = (
    <>
      <span className="fb-row">
        <span className="fb-mark fb-nomark" style={{ width: MARK, height: MARK }} aria-hidden />
        <span className="fb-name">{copy.direct.name}</span>
        <span className="fb-val">{copy.usd0(d.spend)}</span>
      </span>
      <span className="fb-bar" aria-hidden>
        <i className="fb-dshare" style={{ width: pct(d.share) }} />
      </span>
      <span className="fb-dsub">{copy.direct.sub(d.agents, d.harnesses)}</span>
    </>
  )
  if (!hit)
    return (
      <div className={cn('fb-rrow fb-direct', dim(d.id))} data-anchor={d.id} title={label}>
        <span className="sr-only">{label}</span>
        <span aria-hidden className="contents">
          {body}
        </span>
      </div>
    )
  return (
    <Button
      variant="ghost"
      className={cn('fb-hit fb-rowhit fb-rrow fb-direct', lit(d.id))}
      data-node={d.id}
      data-anchor={d.id}
      aria-label={label}
      title={label}
      onClick={() => hit(d.id)}
    >
      {body}
    </Button>
  )
}

/**
 * Providers and models: a section per provider (its mark and name, then its models, each a row with its spend over a
 * bar), every bar on one scale (Ans's reference, 2026-11-04).
 */
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
      {providers.length ? null : (
        <Blank
          text={copy.empty.providers}
          go={{ label: copy.empty.providerKey, to: '/router', search: { tab: 'providers' } }}
        />
      )}
      <div className="fb-prows">
        {providers.map((p) => (
          <div key={p.id} className={cn('fb-prow', dim([p.id, ...p.models.map((m) => m.id)]))}>
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
                <span className="fb-row">
                  <span className="fb-name">{m.name}</span>
                  <span className="fb-val">{copy.usd0(m.spend)}</span>
                </span>
                <span className="fb-bar" aria-hidden>
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

/** A bill line: the name, a dotted leader and the amount. */
function BillRow({
  name,
  amount,
  className,
  title,
  lead,
  anchor,
}: {
  name: string
  amount: string
  className?: string
  title?: string
  lead?: ReactNode
  /** A line lands on it (lines.ts `billLine`). */
  anchor?: string
}) {
  return (
    <span className={cn('fb-bline', className)} title={title} data-anchor={anchor}>
      {lead}
      <span className="fb-name">{name}</span>
      <span className="fb-leader" aria-hidden />
      <span className="fb-val">{amount}</span>
    </span>
  )
}

/**
 * Total spend as a bill: the range's spend added up by provider, then by source, each side coming to the same total
 * (no daily line: Ans, 2026-11-04). A provider's line picks the provider; hovering the bill turns every line it pays
 * for green.
 */
function BillCard({
  bill,
  card,
  hit,
  lit,
  onPaying,
}: {
  bill: Bill
  card: SpendCard
  hit: Hit
  lit: Lit
  onPaying(on: boolean): void
}) {
  const b = copy.bill
  const up = (card.changePct ?? 0) > 0
  return (
    <Card
      id="bill"
      title={copy.totalSpend}
      className="fb-billcard"
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
      {bill.total || bill.providers.length ? (
        <BillLines bill={bill} hit={hit} lit={lit} />
      ) : (
        <Blank text={copy.empty.bill} />
      )}
    </Card>
  )
}

/** The bill's lines: by provider, by source, and the total they add up to. */
function BillLines({ bill, hit, lit }: { bill: Bill; hit: Hit; lit: Lit }) {
  const b = copy.bill
  return (
    <>
      <p className="fb-bhead">{b.byProvider}</p>
      {bill.providers.map((p) => (
        <Button
          key={p.id}
          variant="ghost"
          className={cn('fb-hit fb-rowhit fb-billrow', lit(p.id))}
          data-node={p.id}
          data-anchor={billLine(p.id)}
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
      <BillRow name={b.total} amount={fmtMoney(bill.total)} className="fb-btotal" />
    </>
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
      {/* What no budget governs; with no budget and nothing spent, three zeros say nothing. */}
      {mine || (!b && !hud.ungoverned) ? null : (
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

/**
 * Saved: what optimizations took off the bill, laid out like the bill (the amount, then before and after), with a small
 * turning globe in its corner (Ans's reference, 2026-11-04). The drawer lists the ways to save more, largest first.
 */
function Saved({
  total,
  saved,
  lit,
}: {
  total: number
  saved: number | null
  lit: string | undefined
}) {
  const b = copy.bill
  const off = saved ?? 0
  return (
    <Card id="saved" title={copy.saved} className={cn('fb-savedcard', lit)} count={<Globe />}>
      <span className="fb-amount fb-good">{fmtMoney(off)}</span>
      <p className="fb-sub">{copy.savedSub}</p>
      <span className="fb-rule" aria-hidden />
      {total || off ? (
        <>
          <BillRow name={b.before} amount={fmtMoney(total + off)} />
          <BillRow name={b.total} amount={fmtMoney(total)} className="fb-btotal" />
        </>
      ) : (
        <Blank
          text={copy.empty.saved}
          go={{ label: copy.empty.savedGo, to: '/optimization' }}
          rows={1}
        />
      )}
    </Card>
  )
}

/** A wire globe in the teams' greens, its meridians slowly turning (still under reduced motion). */
function Globe() {
  const r = 92
  return (
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
  )
}
