/**
 * The village canvas (plans/feat-village-overview.md): the Overview's canvas in every view, in the page's existing
 * frame (lens tabs, Mine / Org, range, the sentence). The world fills a dotted canvas; the HUD sits in its corners; Kiko
 * and the Control center say what needs you. Selections are the page's `sel` param, so a link opens the same view.
 *
 * Village and Map share this one canvas: switching view swaps only what is inside the pan-and-zoom zone (the village's
 * world, or the Map's board, `fleet/FleetBoard.tsx`), so the HUD, the Control center, Kiko, the drawer and the zoom
 * stay where they are. Both views have the System card in their top left (`fleet/SystemCard.tsx`, folded to its
 * counts until opened; every part as a list, it replaced the List view); the village has its meters in the top right,
 * the Map a Go to box beside the System card. On the Map a pick doesn't zoom in (its lines reach across the board): the
 * view eases over to make room for the drawer. A click on one of its cards picks the card the same way: it lights, the
 * other cards fade and the drawer opens on the card's insights (`fleet/CardPanel.tsx`), whose buttons go to the pages.
 *
 * Selecting (the prototype's grammar, V31): a click on a place eases the view in just far enough to show it and every
 * place its links reach, a bar at the bottom names it, and the drawer slides in from the right, full height over the
 * village's meters (the map's insight panel for a team, provider, gate, tool or car; the village's own panel for the
 * town hall, token tank, orchestrator, observatory, piggy bank, orb and the project yards). A click on open ground, or
 * Escape, clears it and eases back out to the whole village. The Control center's list opens as a popup under the top
 * edge; the village's bottom centre has the chat box for the Orchestrator (Ask.tsx). The HUD stays where it is.
 *
 * Placing (V28): a Build card lights that kind's open plots green; picking one remembers it in this browser and opens
 * the page where the thing is set up.
 */
import { useNavigate } from '@tanstack/react-router'
import { useReducedMotion } from 'motion/react'
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { Button } from '@nasiko/ui/primitives/button'
import { openGuide } from '@nasiko/features/onboarding/api'
import { cn } from '@nasiko/ui/lib/utils'
import type { OptimisationView } from '@nasiko/features/tokenops/optimisation'
import type { Spend } from '../api'
import type { EstateView } from '../estate/useEstateView'
import { CardPanel } from '../fleet/CardPanel'
import { boardSel, cardOf } from '../fleet/cards'
import { boardCopy } from '../fleet/copy'
import { BOARD, FleetBoard } from '../fleet/FleetBoard'
import { CARDS } from '../fleet/layout'
import type { Anchor } from '../fleet/lines'
import { billSeries, buildBoard } from '../fleet/model'
import { SystemCard } from '../fleet/SystemCard'
import { parseSel } from '../map/selection'
import { OVERLAY, useViewport } from '../map/useViewport'
import { InsightPanel, type PanelProps } from '../panel/InsightPanel'
import { villageCopy as copy } from './copy'
import { P, toMap, up, VILLAGE_FRAME, type Pt } from './geo'
import { fillHeight, FRAME } from './frame'
import { CanvasAsk } from './Ask'
import {
  BuildButton,
  ControlCenter,
  Resources,
  SettingsButton,
  ZoomControls,
  type VillageView,
} from './Hud'
import { Kiko } from './Kiko'
import { BuildersLog } from './Log'
import { buildVillage, LOG_KINDS, type LogKind, type Vacant } from './model'
import { createMotion } from './motion'
import { MotionOn } from './motionContext'
import { PlacePanel } from './PlacePanel'
import { useVillagePrefs } from './prefs'
import { GoTo } from './Query'
import { buildScene, focusFor, placeOf, type Entity } from './scene'
import { Shop, type Dest } from './Shop'
import { hoverStore } from './stores'
import { CARD } from './styles'
import { KIKO_DELAY_MS, PANEL_RESERVE, SELECT_ZOOM, SELECT_ZOOM_ALONE } from './tuning'
import { Chips, World } from './World'
import { useWorld } from './worlds'
import { WorldPicker } from './worlds/WorldPicker'
import type { Kind } from './zones'

export function VillageCanvas({
  v,
  sel,
  onSel,
  view,
  onView,
  panel,
  saved,
  optimisation,
  spend,
  now,
}: {
  v: EstateView
  /** The page's raw `sel` param. */
  sel: string | undefined
  onSel(sel: string | undefined): void
  view: VillageView
  onView(view: VillageView): void
  panel: Omit<PanelProps, 'v' | 'sel'>
  saved: number | null
  /** The range's savings by program: the Map's savings in the drawer. */
  optimisation?: OptimisationView
  /** The Map's bill (its daily line). */
  spend: Spend
  now: number
}) {
  const isMap = view === 'map'
  const mine = v.view.scope === 'mine'
  const prefs = useVillagePrefs()
  const theme = useWorld()
  const village = useMemo(
    () => buildVillage(v, { mine, claims: prefs.claims, saved }),
    [v, mine, prefs.claims, saved],
  )
  const [placing, setPlacing] = useState<{ kind: Kind; name: string; dest: Dest } | null>(null)
  const scene = useMemo(() => buildScene(village, v, { saved }), [village, v, saved])
  // The Map's board, read once for the board and the System card (in both views), and the bill's daily line.
  const board = useMemo(() => buildBoard(v), [v])
  const bill = useMemo(() => billSeries(v.estate, spend, now), [v.estate, spend, now])
  const focus = focusFor(sel, v, scene, village)
  const navigate = useNavigate()

  const reduced = !!useReducedMotion()
  // Nothing moves until the canvas has a size (never in a zero-size layout, such as jsdom).
  const [sized, setSized] = useState(false)
  const motionOn = !reduced && prefs.motion === 'full' && sized
  const [motion] = useState(createMotion)
  useEffect(() => () => motion.dispose(), [motion])
  const [hover] = useState(hoverStore)

  const wrap = useRef<HTMLDivElement>(null)
  const zone = useRef<HTMLDivElement>(null)
  const [height, setHeight] = useState<number | null>(null)
  const [wide, setWide] = useState(false)
  useLayoutEffect(() => {
    const el = wrap.current
    if (!el) return
    const measure = () => {
      const w = el.clientWidth
      setWide(w >= OVERLAY)
      setSized(w > 0)
      setHeight(fillHeight(el))
    }
    measure()
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  // The drawer keeps showing the last selection while it slides out.
  const [shown, setShown] = useState(sel)
  if (sel && sel !== shown) setShown(sel)
  const [details, setDetails] = useState(true)
  const open = !!sel && details
  // The Control center's list: a popup under the top edge, over the canvas.
  const [logOpen, setLogOpen] = useState(false)
  const [logTab, setLogTab] = useState<LogKind>('fix')
  const drawerOpen = open
  // The System card, folded to its counts until opened.
  const [systemOpen, setSystemOpen] = useState(false)
  const reserve = wide && drawerOpen ? PANEL_RESERVE : 0
  const changeView = (x: VillageView) => onView(x)
  const vp = useViewport(zone, isMap ? BOARD : VILLAGE_FRAME, reserve, {
    anywhere: true,
    easeReserve: isMap && !reduced,
  })
  // A new view starts fitted: the zone now holds another world, at another size.
  const fitNow = vp.fit
  useEffect(() => fitNow(), [view, fitNow])
  /** Where the Map's parts are on its board (FleetBoard measures them). */
  const anchors = useRef<ReadonlyMap<string, Anchor>>(new Map())
  const [onAnchors] = useState(() => (at: ReadonlyMap<string, Anchor>) => {
    anchors.current = at
  })

  /** Where a node is drawn now, in map units (a car where it drives, else the place that stands for it). */
  const pointOf = (anchor: string | null): Pt | null => {
    if (!anchor) return null
    if (isMap) {
      const b = anchors.current.get(anchor)
      return b ? [b.x + b.w / 2, b.y + b.h / 2] : null
    }
    const at = motion.at(anchor)
    if (at && village.cars.some((t) => t.id === anchor)) return toMap(up(P(at[0], at[1]), 14))
    const e = scene.find((x) => x.sel && x.nodes.includes(anchor))
    return e ? toMap([e.top[0], e.top[1] + 40]) : null
  }
  /**
   * Eases the view to a selection (after the drawer opens, which changes the free width it centres in), just far
   * enough to show it and every place its links reach, arcs included (World draws the same links to the same places).
   * A place with no links eases in to SELECT_ZOOM_ALONE × the fitted view; nothing goes past SELECT_ZOOM.
   */
  const zoomTo = (s: string) => {
    const f = focusFor(s, v, scene, village)
    const e = scene.find((x) => x.sel === s) ?? scene.find((x) => x.key === f.key)
    const id = parseSel(s)?.type === 'node' ? s.slice(5) : null
    const car = f.key?.startsWith('car:') ? motion.at(f.key.slice(4)) : null
    // Where the links start, in world px: the place's top, or the car where it drives.
    const from: Pt | null = car ? up(P(car[0], car[1]), 24) : e ? e.top : null
    const keep = f.keep
    const targets = keep
      ? scene
          .filter((x) => x.sel && x.key !== f.key && x.nodes.some((n) => keep.has(n)))
          .slice(0, 18)
      : []
    const box: Pt[] = []
    const add = (x: Entity) => {
      const { i, j, w, d } = x.foot
      for (const p of [P(i, j), P(i + w, j), P(i, j + d), P(i + w, j + d), up(x.top, 18)])
        box.push(toMap(p))
    }
    if (e && !car) add(e)
    if (from) box.push(toMap(from))
    for (const t of targets) {
      add(t)
      if (!from) continue
      // The arc's highest point (World's link curve).
      const dist = Math.hypot(from[0] - t.top[0], from[1] - t.top[1])
      box.push(toMap([(from[0] + t.top[0]) / 2, Math.min(from[1], t.top[1]) - 30 - dist * 0.14]))
    }
    if (!box.length) {
      const pt = pointOf(id)
      if (pt) box.push(pt)
    }
    if (!box.length) return
    const xs = box.map((p) => p[0])
    const ys = box.map((p) => p[1])
    const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)]
    setTimeout(() => {
      const el = zone.current
      if (!el) return
      const fitted = vp.view().z / Math.max(0.01, vp.pct / 100)
      // Clear of the drawer, the HUD's top tabs and the bottom bar with the query box.
      const free = el.clientWidth - (wide ? PANEL_RESERVE : 0)
      const [top, bottom, side] = [64, 120, 40]
      const want = Math.min(
        (free - side * 2) / Math.max(1, x1 - x0),
        (el.clientHeight - top - bottom) / Math.max(1, y1 - y0),
      )
      const z = Math.min(fitted * (targets.length ? SELECT_ZOOM : SELECT_ZOOM_ALONE), want)
      vp.center((x0 + x1) / 2, (y0 + y1) / 2 + (bottom - top) / 2 / z, z, reduced)
    }, 60)
  }
  /** The Map's part a selection is about: its row, a harness install's column, the budget's gauge, a picked card. */
  const boxOf = (s: string): Anchor | undefined => {
    const c = cardOf(boardSel(s))
    if (c) {
      const b = CARDS[c]
      return { x: b.left, y: b.top, w: b.width, h: b.height }
    }
    const id = s.startsWith('node:') ? s.slice('node:'.length) : null
    if (!id) return s === 'place:tank' ? anchors.current.get('org') : undefined
    const h = v.estate.harnesses.find((x) => x.id === id)
    return anchors.current.get(h ? `hk:${h.harness}` : id)
  }
  /** Pans the Map, at its zoom, just enough that a part isn't under the drawer or past an edge. */
  const reveal = (b: Anchor | undefined) => {
    const el = zone.current
    if (!b || !el) return
    const { x, y, z } = vp.view()
    const free = el.clientWidth - (wide ? PANEL_RESERVE : 0)
    const h = el.clientHeight
    const [x0, x1, y0, y1] = [
      (b.x - x) * z,
      (b.x + b.w - x) * z,
      (b.y - y) * z,
      (b.y + b.h - y) * z,
    ]
    let dx = x1 > free - 24 ? x1 - (free - 24) : 0
    if (x0 - dx < 16) dx = x0 - 16
    let dy = y1 > h - 72 ? y1 - (h - 72) : 0
    if (y0 - dy < 64) dy = y0 - 64
    if (dx || dy) vp.center(x + (free / 2 + dx) / z, y + (h / 2 + dy) / z, z, reduced)
  }
  const select = (s: string) => {
    setLogOpen(false)
    setKikoOpen(false)
    setDetails(true)
    if (isMap) {
      // The board keeps its zoom (its lines reach across it): only what the drawer would cover moves into view.
      vp.hold()
      onSel(s)
      setTimeout(() => reveal(boxOf(s)), 60)
      return
    }
    onSel(s)
    zoomTo(s)
  }
  const clear = () => {
    if (placing) return setPlacing(null)
    if (!sel) return
    onSel(undefined)
    if (isMap) setTimeout(() => vp.fitSmooth(reduced), 60)
    else vp.fitSmooth(reduced)
  }

  // Escape: placing first, then a popover or the log, then the selection.
  const [kikoOpen, setKikoOpen] = useState(false)
  const [kikoIndex, setKikoIndex] = useState(0)
  const [shopOpen, setShopOpen] = useState(false)
  useEffect(() => {
    const el = wrap.current
    if (!el) return
    const key = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return
      if (placing) setPlacing(null)
      else if (logOpen) setLogOpen(false)
      else if (kikoOpen) setKikoOpen(false)
      else if (sel) {
        onSel(undefined)
        if (isMap) setTimeout(() => vp.fitSmooth(reduced), 60)
        else vp.fitSmooth(reduced)
      } else return
      e.preventDefault()
    }
    el.addEventListener('keydown', key)
    return () => el.removeEventListener('keydown', key)
  }, [placing, logOpen, kikoOpen, sel, onSel, vp, reduced, isMap])

  // Kiko speaks up once per visit, a moment after the village loads, when something is broken: not over a link that
  // opened a selection, and not on a phone-sized canvas, where the bubble would cover the village.
  const broken = village.log.fix.length > 0
  const [spoke, setSpoke] = useState(() => !!sel)
  useEffect(() => {
    if (spoke || !broken) return
    const t = setTimeout(() => {
      setSpoke(true)
      if ((wrap.current?.clientWidth ?? 0) >= 640) setKikoOpen((o) => o || !logOpen)
    }, KIKO_DELAY_MS)
    return () => clearTimeout(t)
  }, [spoke, broken, logOpen])

  const take = (item: { sel: string | null; anchor: string | null }) => {
    setLogOpen(false)
    setDetails(true)
    if (item.sel) onSel(item.sel)
    // On the village a place opens as a click on it does: the view fits it and its links.
    if (!isMap && item.sel) {
      zoomTo(item.sel)
      return
    }
    if (isMap) vp.hold()
    const pt = pointOf(item.anchor)
    if (pt) setTimeout(() => vp.center(pt[0], pt[1], undefined, reduced), 60)
  }

  // Placing: what exists of each kind now, so the plot picked goes to the next new one.
  const e = v.estate
  const knownOf = (k: Kind): string[] =>
    ({
      team: e.groups.map((g) => g.id),
      project: [...(e.projects ?? []).map((p) => p.id), 'unfiled'],
      mine: e.providers.map((p) => p.id),
      xmine: e.providers.map((p) => p.id),
      gate: e.routers.map((r) => r.id),
      bay: e.agents.map((a) => a.id),
      tool: e.tools.map((t) => t.id),
      mcp: e.tools.map((t) => t.id),
      harness: [...new Set(e.harnesses.map((h) => h.harness))],
    })[k]
  const pick = (p: Vacant) => {
    if (!placing) return
    prefs.claim(p.kind, { plot: p.plot, known: knownOf(p.kind) })
    const dest = placing.dest
    setPlacing(null)
    void navigate(dest)
  }

  const shownPlace = placeOf(shown)
  const parsed = parseSel(shown)
  const shownBoard = boardSel(shown)
  const inspector = shownPlace ? (
    <PlacePanel place={shownPlace} v={v} village={village} saved={saved} onClose={() => clear()} />
  ) : shownBoard ? (
    <CardPanel
      s={shownBoard}
      v={v}
      board={board}
      card={bill.card}
      dates={bill.dates}
      spend={spend}
      hud={village.hud}
      mine={mine}
      optimisation={optimisation}
      onPick={select}
      onClose={() => clear()}
    />
  ) : parsed ? (
    <InsightPanel v={v} sel={parsed} {...panel} />
  ) : null
  const counts = Object.fromEntries(LOG_KINDS.map((k) => [k, village.log[k].length])) as Record<
    LogKind,
    number
  >
  const drawer = open ? inspector : null
  const selected = focus.key ? scene.find((x) => x.key === focus.key) : undefined
  const selName = isMap
    ? mapName(sel, v, mine)
    : (selected?.label ??
      (focus.key?.startsWith('car:')
        ? village.cars.find((c) => `car:${c.id}` === focus.key)?.name
        : undefined))

  // The System card: top left in both views, folded to its counts; under the canvas when it is too narrow to share.
  const systemUi = (
    <SystemCard
      v={v}
      board={board}
      sel={sel}
      onPick={select}
      open={systemOpen}
      onOpen={setSystemOpen}
      className={wide ? 'pointer-events-auto max-h-full' : 'max-h-[480px] w-full'}
    />
  )

  // A theme change redraws the world; make sure the SVG (and its hit targets) sits at the current view right away.
  useLayoutEffect(() => {
    if (!isMap) vp.reapply()
    // eslint-disable-next-line react-hooks/exhaustive-deps -- only on a theme change
  }, [theme.id])

  return (
    <MotionOn.Provider value={motionOn}>
      <div
        className="@container/village motion-safe:animate-in motion-safe:duration-300 motion-safe:fade-in"
        data-testid="village-canvas"
      >
        <div
          ref={wrap}
          className={FRAME}
          style={{ ...(height ? { height } : null), ...(isMap ? null : theme.backdrop) }}
        >
          <div
            ref={zone}
            // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- the village pans and zooms from the keyboard, so it takes focus
            tabIndex={0}
            role="application"
            aria-label={isMap ? boardCopy.instructions : copy.instructions}
            className="absolute inset-0 cursor-grab touch-none outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset data-[dragging]:cursor-grabbing"
          >
            {isMap ? (
              <FleetBoard
                v={v}
                board={board}
                card={bill.card}
                dates={bill.dates}
                hud={village.hud}
                mine={mine}
                saved={saved}
                sel={sel}
                onSelect={select}
                onClear={clear}
                onBackground={clear}
                subscribe={vp.subscribe}
                viewNow={vp.view}
                running={motionOn && prefs.flow}
                onAnchors={onAnchors}
              />
            ) : (
              <World
                village={village}
                scene={scene}
                v={v}
                focus={focus}
                onSelect={select}
                onBackground={clear}
                motion={motion}
                motionOn={motionOn}
                flow={prefs.flow}
                grid={prefs.grid}
                hover={hover}
                placing={placing?.kind ?? null}
                placingName={placing?.name ?? ''}
                onPlot={pick}
                subscribe={vp.subscribe}
                viewNow={vp.view}
                saved={saved}
              />
            )}
          </div>
          {isMap ? null : (
            <Chips
              scene={scene}
              focus={focus}
              v={v}
              labels={prefs.labels}
              hover={hover}
              view={vp}
            />
          )}

          {/* Top centre, hanging from the edge: tabs of one style. They stay put whatever opens beside them. The view
              switch is the page header's (Overview · Village · Map), so the canvas has none of its own. */}
          <div className="absolute top-0 left-1/2 z-10 flex -translate-x-1/2 items-start gap-1.5">
            <ControlCenter
              counts={counts}
              open={logOpen}
              tab={logTab}
              onToggle={() => {
                setKikoOpen(false)
                setLogOpen((o) => !o)
              }}
              onTab={(k) => {
                setKikoOpen(false)
                setLogTab(k)
                setLogOpen(true)
              }}
              action={isMap ? undefined : <BuildButton onShop={() => setShopOpen(true)} />}
            />
            <ZoomControls
              onZoomIn={vp.zoomIn}
              onZoomOut={vp.zoomOut}
              onFit={() => vp.fitSmooth(reduced)}
            />
            <SettingsButton reducedBySystem={reduced} onReplay={() => openGuide('welcome')} />
          </div>
          {/* Top left, in both views: the System card (and on the Map, Go to beside it). */}
          {wide ? (
            <div
              data-vp-ignore=""
              className="pointer-events-none absolute top-3 bottom-24 left-3 z-10 flex items-start gap-2"
            >
              {systemUi}
              {isMap ? (
                <div className="pointer-events-auto">
                  <GoTo v={v} onPick={select} />
                </div>
              ) : null}
            </div>
          ) : null}
          {/* Top right: the village's meters (the Map's board shows the same numbers on its own cards). They stay put;
              the drawer opens over them, since what was picked is what the person wants to read then. */}
          {isMap ? null : (
            <div
              aria-hidden={wide && drawerOpen}
              inert={wide && drawerOpen}
              className="absolute top-3 right-3 z-10 hidden @[560px]/village:block"
            >
              <Resources
                hud={village.hud}
                saved={saved}
                mine={mine}
                onPlace={(id) => take({ sel: `place:${id}`, anchor: null })}
              />
            </div>
          )}
          <div className="absolute bottom-2 left-3 z-10">
            <Kiko
              log={village.log}
              open={kikoOpen}
              index={kikoIndex}
              onToggle={() => {
                setLogOpen(false)
                setKikoOpen((o) => !o)
              }}
              onNext={() => setKikoIndex((k) => k + 1)}
              onClose={() => setKikoOpen(false)}
              onTake={take}
              estate={v.estate}
            />
          </div>

          {isMap ? null : (
            <div className="absolute right-3 bottom-3 z-10 hidden @[900px]/village:block">
              <WorldPicker />
            </div>
          )}

          {/* Bottom centre, fixed like the top: the placing hint or the selection's name with Details / Hide and
              Deselect, over the village's chat box. */}
          <div
            data-vp-ignore=""
            className="absolute bottom-3 left-1/2 z-10 flex w-[min(440px,calc(100%-176px))] -translate-x-1/2 flex-col items-center gap-2"
          >
            {placing ? (
              <div className={cn(CARD, 'flex items-center gap-3 px-3 py-2 text-sm')} role="status">
                <span>
                  {copy.placing.hint(placing.name)}{' '}
                  <span className="text-muted-foreground">{copy.placing.locked}</span>
                </span>
                <Button size="sm" variant="outline" onClick={() => setPlacing(null)}>
                  {copy.placing.cancel}
                </Button>
              </div>
            ) : sel && selName ? (
              <div className={cn(CARD, 'flex items-center gap-2 py-1.5 pr-1.5 pl-3 text-sm')}>
                <span className="max-w-56 truncate font-medium">{selName}</span>
                <Button size="sm" variant="outline" onClick={() => setDetails((d) => !d)}>
                  {details ? copy.selection.hide : copy.selection.details}
                </Button>
                <Button size="sm" variant="ghost" onClick={clear}>
                  {copy.selection.close}
                </Button>
              </div>
            ) : null}
            {isMap ? null : <CanvasAsk className="w-full" />}
          </div>

          {/* The Control center's list: a popup under the top edge, as it first was. */}
          {logOpen ? (
            <div
              className={cn(
                'pointer-events-none absolute inset-x-3 top-14 z-20 flex justify-center *:pointer-events-auto',
                isMap ? 'bottom-3' : 'bottom-20',
              )}
            >
              <BuildersLog
                log={village.log}
                tab={logTab}
                onTab={setLogTab}
                estate={v.estate}
                onTake={take}
                onClose={() => setLogOpen(false)}
              />
            </div>
          ) : null}
          {wide ? (
            <aside
              data-vp-ignore=""
              aria-hidden={!drawer}
              inert={!drawer}
              className={cn(
                'absolute top-3 right-3 bottom-3 z-20 w-[360px] overflow-hidden rounded-lg border bg-card shadow-md transition-[translate] duration-[240ms] ease-out motion-reduce:transition-none',
                drawer ? 'translate-x-0' : 'translate-x-[calc(100%+28px)]',
              )}
            >
              {drawer ?? inspector}
            </aside>
          ) : null}
        </div>
        {wide ? null : (
          <div className="mt-3 flex flex-col gap-2">
            {isMap ? <GoTo v={v} onPick={select} /> : null}
            {systemUi}
          </div>
        )}
        {!wide && open && inspector ? (
          <aside className="mt-3 max-h-[520px] overflow-hidden rounded-xl border bg-card">
            {inspector}
          </aside>
        ) : null}
        <Shop
          open={shopOpen}
          onOpenChange={setShopOpen}
          motion={motionOn}
          canPlace={(k) => village.vacant.some((p) => p.kind === k && p.open)}
          onPlace={(kind, name, dest) => {
            onSel(undefined)
            // Plots are the village's: placing from the Map goes back to it.
            if (view !== 'village') changeView('village')
            setPlacing({ kind, name, dest })
          }}
        />
      </div>
    </MotionOn.Provider>
  )
}

/** The Map's selection, named in the bar at the bottom: the part, the card, or the village place it opens. */
function mapName(sel: string | undefined, v: EstateView, mine: boolean): string | undefined {
  if (sel?.startsWith('node:')) return v.nodes.get(sel.slice('node:'.length))?.name
  if (sel?.startsWith('opp:'))
    return v.estate.opportunities.find((o) => o.id === sel.slice('opp:'.length))?.title
  const b = boardSel(sel)
  if (b?.type === 'day') return boardCopy.totalSpend
  if (b) {
    const names: Record<string, string> = {
      teams: boardCopy.teams,
      agents: boardCopy.agents,
      workflows: boardCopy.workflows,
      routers: boardCopy.routers,
      providers: boardCopy.providers,
      tools: boardCopy.tools,
      harness: boardCopy.harness,
      bill: boardCopy.totalSpend,
      saved: boardCopy.saved,
      opportunities: boardCopy.opps.title,
      budget: mine ? boardCopy.allowance : boardCopy.budget,
    }
    return names[b.id]
  }
  const place = placeOf(sel)
  if (place === 'tank') return copy.place.tank
  if (place === 'piggy') return copy.place.piggy
  if (place === 'orb') return copy.place.orb
  return undefined
}
