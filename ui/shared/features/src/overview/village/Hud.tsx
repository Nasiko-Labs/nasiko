/**
 * The canvas's heads-up display (plans/feat-village-overview.md §4.10), in its corners:
 *
 * - top centre, hanging from the edge as tabs of one style (`HUD_TAB`), fixed whatever opens beside them:
 *   Village · Map, the Control center (its name over what needs you, a plain count per kind that opens its tab; on the
 *   village also Build, the shop), zoom and fit, settings;
 * - top right, on the village only: three resource meters, Clash-style, on one scale (the budget, V57) with the org's
 *   trend under each (V59): budget used (gold, opens the token tank), saved (elixir), ungoverned (dark elixir: no
 *   budget, or one that can only alert, V58). The drawer opens over them. The Map shows the same numbers on its board;
 * - top left, in both views: the System card, folded to its counts (fleet/SystemCard.tsx).
 *
 * Kiko (bottom left) is `Kiko.tsx`. Every control is a shadcn primitive on theme tokens; the meters' fills and icons
 * keep the game's resource colours (`palette.ts`) because they are the picture's resources, not UI colour.
 */
import { Maximize2, Minus, Plus, Settings2 } from 'lucide-react'
import type { ReactNode } from 'react'
import { setTheme, useThemePrefs, type Theme } from '@nasiko/app-core/shell/theme'
import { Button } from '@nasiko/ui/primitives/button'
import { Popover, PopoverContent, PopoverTrigger } from '@nasiko/ui/primitives/popover'
import { ToggleGroup, ToggleGroupItem } from '@nasiko/ui/primitives/toggle-group'
import { fmtMoney } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import { Spark } from '../components/Kpis'
import { villageCopy as copy } from './copy'
import { meterScale, type Hud, type LogKind, type Pose } from './model'
import { useVillagePrefs } from './prefs'
import { CARD, HUD_TAB } from './styles'

/** A pose as a shape (shape carries the meaning; colour backs it up). */
export function PoseMark({ pose, className }: { pose: Pose; className?: string }) {
  return (
    <svg viewBox="0 0 14 14" aria-hidden className={cn('size-3.5 shrink-0', className)}>
      {pose === 'live' ? <circle cx={7} cy={7} r={5} className="fill-success" /> : null}
      {pose === 'idle' ? (
        <path
          d="M3 3.5h8L3 10.5h8"
          fill="none"
          strokeWidth={2}
          strokeLinejoin="round"
          className="stroke-muted-foreground"
        />
      ) : null}
      {pose === 'hurt' ? (
        <>
          <path d="M7 1.8l5.6 10H1.4z" className="fill-warning" />
          <path d="M7 5.5v3" stroke="white" strokeWidth={1.5} />
        </>
      ) : null}
      {pose === 'down' ? (
        <>
          <circle cx={7} cy={7} r={6} className="fill-destructive" />
          <path d="M4.6 4.6l4.8 4.8M9.4 4.6L4.6 9.4" stroke="white" strokeWidth={1.6} />
        </>
      ) : null}
    </svg>
  )
}

export type VillageView = 'village' | 'map'

/**
 * The Control center: the canvas's global control, in the middle of the top edge, in its first layout (Ans,
 * 2026-10-17: small, no stroke, no tinted badges). Its name, and under it what needs you as plain counts (the number in
 * the foreground, red only for what is broken); the name opens the log of everything, a count opens its own tab. Beside
 * them the view's call to action (`action`), when it has one.
 */
export function ControlCenter({
  counts,
  open,
  tab,
  onToggle,
  onTab,
  action,
}: {
  counts: Record<LogKind, number>
  open: boolean
  /** The log's tab while it is open: its count reads as pressed. */
  tab: LogKind
  onToggle(): void
  /** Opens the log on one tab (a count). */
  onTab(k: LogKind): void
  action?: ReactNode
}) {
  return (
    <div className={cn(HUD_TAB, 'gap-2 pr-2 pl-3')}>
      <div className="flex flex-col justify-center">
        <Button
          variant="ghost"
          className="h-auto justify-start p-0 text-xs leading-4 font-semibold hover:bg-transparent hover:underline"
          aria-expanded={open}
          aria-label={copy.hud.logLabel(counts)}
          onClick={onToggle}
        >
          {copy.hud.log}
        </Button>
        <span className="hidden items-center gap-2 text-xs leading-4 text-muted-foreground tabular-nums @[560px]/village:flex">
          {(['fix', 'approve', 'watch', 'upgrade'] as const).map((k) => (
            <Button
              key={k}
              variant="ghost"
              aria-label={copy.hud.countLabel(k, counts[k])}
              aria-pressed={open && tab === k}
              onClick={() => onTab(k)}
              className={cn(
                'h-auto gap-1 rounded-sm p-0 text-xs leading-4 font-normal text-muted-foreground hover:bg-transparent hover:text-foreground',
                open && tab === k && 'text-foreground underline underline-offset-2',
              )}
            >
              <b
                className={cn(
                  'font-semibold text-foreground',
                  k === 'fix' && counts.fix > 0 && 'text-destructive',
                )}
              >
                {counts[k]}
              </b>
              {copy.logShort[k]}
            </Button>
          ))}
        </span>
      </div>
      {action}
    </div>
  )
}

export function SettingsButton({
  reducedBySystem,
  onReplay,
}: {
  reducedBySystem: boolean
  /** Replay the intro: the onboarding, again from what brings you here. */
  onReplay(): void
}) {
  const p = useVillagePrefs()
  const theme = useThemePrefs().theme
  const row = <K extends 'labels' | 'motion'>(k: K, opts: Record<string, string>) => (
    <ToggleGroup
      size="sm"
      variant="outline"
      value={[p[k]]}
      onValueChange={(vs) => {
        const x = vs[0] as string
        return x && p.set({ [k]: x } as never)
      }}
      aria-label={k === 'labels' ? copy.settings.labels : copy.settings.motion}
    >
      {Object.entries(opts).map(([v, l]) => (
        <ToggleGroupItem key={v} value={v} className="px-2.5 text-xs">
          {l}
        </ToggleGroupItem>
      ))}
    </ToggleGroup>
  )
  const onOff = (k: 'flow' | 'grid', label: string) => (
    <ToggleGroup
      size="sm"
      variant="outline"
      value={[p[k] ? 'on' : 'off']}
      onValueChange={(vs) => {
        const x = vs[0] as string
        return x && p.set({ [k]: x === 'on' })
      }}
      aria-label={label}
    >
      <ToggleGroupItem value="on" className="px-2.5 text-xs">
        {copy.settings.on}
      </ToggleGroupItem>
      <ToggleGroupItem value="off" className="px-2.5 text-xs">
        {copy.settings.off}
      </ToggleGroupItem>
    </ToggleGroup>
  )
  return (
    <Popover>
      <div className={HUD_TAB}>
        <PopoverTrigger
          render={
            <Button variant="ghost" size="icon" className="size-8" aria-label={copy.hud.settings} />
          }
        >
          <Settings2 aria-hidden />
        </PopoverTrigger>
      </div>
      <PopoverContent align="center" className="w-80">
        <p className="mb-1 text-xs font-semibold text-muted-foreground">{copy.settings.map}</p>
        <div className="flex flex-col gap-2 text-sm">
          <div className="flex items-center justify-between gap-3">
            <span>{copy.settings.labels}</span>
            {row('labels', copy.settings.labelOpts)}
          </div>
          <div className="flex items-center justify-between gap-3">
            <span>{copy.settings.motion}</span>
            {row('motion', copy.settings.motionOpts)}
          </div>
          {reducedBySystem ? (
            <p className="text-xs text-muted-foreground">{copy.settings.reducedBySystem}</p>
          ) : null}
          <div className="flex items-center justify-between gap-3">
            <span>{copy.settings.flow}</span>
            {onOff('flow', copy.settings.flow)}
          </div>
          <div className="flex items-center justify-between gap-3">
            <span>{copy.settings.grid}</span>
            {onOff('grid', copy.settings.grid)}
          </div>
          <div className="flex items-center justify-between gap-3">
            <span>{copy.settings.theme}</span>
            <ToggleGroup
              size="sm"
              variant="outline"
              value={[theme]}
              onValueChange={(vs) => {
                const x = vs[0] as string
                return x && setTheme(x as Theme)
              }}
              aria-label={copy.settings.theme}
            >
              {Object.entries(copy.settings.themeOpts).map(([v, l]) => (
                <ToggleGroupItem key={v} value={v} className="px-2.5 text-xs">
                  {l}
                </ToggleGroupItem>
              ))}
            </ToggleGroup>
          </div>
        </div>
        <p className="mt-3 mb-1 text-xs font-semibold text-muted-foreground">
          {copy.settings.village}
        </p>
        <div className="flex flex-wrap gap-2">
          <Button size="sm" variant="outline" onClick={onReplay}>
            {copy.settings.replay}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={!Object.keys(p.claims).length}
            onClick={() => p.set({ claims: {} })}
          >
            {copy.settings.resetLayout}
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  )
}

const RES = {
  gold: {
    fill: 'linear-gradient(90deg,#ffe17a,#f2b322)',
    line: { fill: '#f6c94a', edge: '#a87e12' },
    icon: <circle cx={9} cy={9} r={7} fill="#f6c94a" stroke="#a87e12" strokeWidth={1.4} />,
  },
  elixir: {
    fill: 'linear-gradient(90deg,#f6a6e4,#c24fd0)',
    line: { fill: '#e58ce0', edge: '#a8329f' },
    icon: (
      <path
        d="M9 1.5c3 4.5 6.3 7 6.3 10.8a6.3 6.3 0 0 1-12.6 0C2.7 8.5 6 6 9 1.5z"
        fill="#d45ad6"
        stroke="#842a86"
        strokeWidth={1.2}
      />
    ),
  },
  dark: {
    fill: 'linear-gradient(90deg,#8a76a6,#3b2d50)',
    line: { fill: '#8a76a6', edge: '#e5484d' },
    icon: (
      <path
        d="M9 1.5c3 4.5 6.3 7 6.3 10.8a6.3 6.3 0 0 1-12.6 0C2.7 8.5 6 6 9 1.5z"
        fill="#3b2d50"
        stroke="#e5484d"
        strokeWidth={1.4}
      />
    ),
  },
} as const

/**
 * A resource meter. Every meter's fill is a share of the same scale (V57), so the three bars compare at a glance; a
 * non-zero share always shows a sliver, and `share` null draws no bar (nothing to measure it against). The trend line
 * under it is the org's spend per day (V59), decorative: the figure carries the number.
 */
function Meter({
  label,
  value,
  share,
  pct,
  res,
  trend,
  caption,
  bad,
  onClick,
}: {
  label: string
  value: string
  share: number | null
  /** The share as words ("59%"), when the scale is a budget. */
  pct: string | null
  res: keyof typeof RES
  trend: readonly number[] | null
  caption: string
  bad?: boolean
  onClick(): void
}) {
  const width = share === null ? 0 : share > 0 ? Math.max(0.02, Math.min(1, share)) : 0
  return (
    <Button
      variant="ghost"
      className={cn(
        CARD,
        'h-auto w-56 flex-col items-stretch gap-1 px-2.5 py-1.5 text-left hover:bg-card',
      )}
      aria-label={`${label}: ${value}${pct ? `, ${copy.hud.ofBudget(pct)}` : ''}. ${caption}`}
      onClick={onClick}
    >
      <span className="flex items-center gap-2">
        <svg viewBox="0 0 18 18" aria-hidden className="size-4 shrink-0">
          {RES[res].icon}
        </svg>
        <span className="flex-1 text-xs font-medium text-muted-foreground">{label}</span>
        {pct ? (
          <span
            className={cn(
              'text-2xs font-medium text-muted-foreground tabular-nums',
              share !== null && share > 1 && 'text-destructive',
            )}
          >
            {pct}
          </span>
        ) : null}
        <span className="text-sm font-semibold tabular-nums">{value}</span>
      </span>
      {share === null ? null : (
        <span className="h-1.5 overflow-hidden rounded-full bg-muted">
          <span
            className="block h-full rounded-full"
            style={{ width: `${Math.round(width * 100)}%`, background: RES[res].fill }}
          />
        </span>
      )}
      {trend ? (
        // `size-full` in a sized box: the Button sizes any svg without a `size-` class to an icon's 16 px.
        <span className="block h-4">
          <Spark
            points={trend.map((v, i) => ({ key: String(i), value: v }))}
            kind="line"
            series={RES[res].line}
            className="mt-0 size-full"
          />
        </span>
      ) : null}
      <span
        className={cn(
          'hidden truncate text-2xs font-normal text-muted-foreground @[760px]/village:block',
          bad && 'font-medium text-destructive',
        )}
      >
        {caption}
      </span>
    </Button>
  )
}

export function Resources({
  hud,
  saved,
  mine,
  onPlace,
}: {
  hud: Hud
  saved: number | null
  mine: boolean
  onPlace(id: 'tank' | 'piggy'): void
}) {
  const b = hud.budget
  const scale = meterScale(hud)
  const shareOf = (value: number) => (scale ? value / scale : null)
  const pctOf = (value: number) => (b && scale ? copy.hud.pct(value / scale) : null)
  const t = hud.trend
  return (
    <div className="flex flex-col items-end gap-2">
      <Meter
        label={mine ? copy.hud.allowanceUsed : copy.hud.budgetUsed}
        value={b ? fmtMoney(b.spent) : '—'}
        share={b ? shareOf(b.spent) : 0}
        pct={b ? pctOf(b.spent) : null}
        res="gold"
        trend={t?.spent ?? null}
        caption={b ? copy.hud.paceOf(b.pace, b.limit) : copy.hud.noBudget}
        bad={!!b && b.pace > 1}
        onClick={() => onPlace('tank')}
      />
      {/* Saved is the workspace's even in Mine, so it is measured against the allowance only in Org. */}
      <Meter
        label={copy.hud.saved}
        value={saved ? fmtMoney(saved) : '$0'}
        share={mine ? null : shareOf(saved ?? 0)}
        pct={mine ? null : pctOf(saved ?? 0)}
        res="elixir"
        trend={t?.saved ?? null}
        caption={saved ? copy.hud.savedCap : copy.hud.savedEmpty}
        onClick={() => onPlace('piggy')}
      />
      {mine ? null : (
        <Meter
          label={copy.hud.ungoverned}
          value={fmtMoney(hud.ungoverned)}
          share={shareOf(hud.ungoverned)}
          pct={pctOf(hud.ungoverned)}
          res="dark"
          trend={t?.ungoverned ?? null}
          caption={hud.ungoverned > 0 ? copy.hud.ungovernedCap : copy.hud.ungovernedNone}
          bad={hud.ungoverned > 0}
          onClick={() => onPlace('tank')}
        />
      )}
    </div>
  )
}

/** Build: the Control center's call to action on the village, the app's primary button (it opens the shop). */
export function BuildButton({ onShop }: { onShop(): void }) {
  return (
    <Button size="sm" className="h-8" aria-label={copy.hud.buildLabel} onClick={onShop}>
      {copy.hud.build}
    </Button>
  )
}

/** Zoom and fit, a tab on the canvas's top edge beside settings. */
export function ZoomControls({
  onZoomIn,
  onZoomOut,
  onFit,
}: {
  onZoomIn(): void
  onZoomOut(): void
  onFit(): void
}) {
  return (
    <div className={cn(HUD_TAB, 'gap-0.5')}>
      <Button
        variant="ghost"
        size="icon"
        className="size-8"
        aria-label={copy.hud.zoomOut}
        onClick={onZoomOut}
      >
        <Minus aria-hidden />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className="size-8"
        aria-label={copy.hud.zoomIn}
        onClick={onZoomIn}
      >
        <Plus aria-hidden />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className="size-8"
        aria-label={copy.hud.fit}
        onClick={onFit}
      >
        <Maximize2 aria-hidden />
      </Button>
    </div>
  )
}
