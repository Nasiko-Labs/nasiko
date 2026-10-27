/**
 * The Build shop (plans/feat-village-overview.md V5, V28). A card that has a district starts placing: the shop closes,
 * that kind's open plots turn green, and picking one opens the page where the thing is set up (it lands on that plot
 * once it exists). A card without a district (Opportunities, the rate-limit tower) opens its page at once. Things
 * OpenRuntime doesn't have yet (team plots in this edition, Route, walls and checkpoints) say so instead of pretending.
 */
import { Link } from '@tanstack/react-router'
import { Gauge, Milestone, ShieldCheck } from 'lucide-react'
import { useState, type ReactNode } from 'react'
import { Button, asButton } from '@nasiko/ui/primitives/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@nasiko/ui/primitives/dialog'
import { Tabs, TabsList, TabsTrigger } from '@nasiko/ui/primitives/tabs'
import { GateArt, MineArt, OpportunityArt, Rune, TeamArt } from './art'
import { villageCopy as copy } from './copy'
import { HH, HW } from './geo'
import { MotionOn } from './motionContext'
import { HarnessBall, McpHouse, PieceSprite, ToolBox } from './places'
import type { Kind } from './zones'

type Tab = keyof typeof copy.shop.tabs
export type Dest =
  | { to: '/deploy' }
  | { to: '/mcp' }
  | { to: '/router' }
  | { to: '/harnesses' }
  | { to: '/optimization' }
  | { to: '/settings' }
interface Item {
  id: string
  name: string
  desc: string
  art: ReactNode
  dest?: Dest
  page?: string
  /** The district it is placed in. */
  place?: Kind
}

const frame = (w: number, d: number, h: number, children: ReactNode) => (
  <svg
    viewBox={`${-d * HW - 16} ${-h - 22} ${(w + d) * HW + 32} ${(w + d) * HH + h + 34}`}
    aria-hidden
    className="size-full"
  >
    {children}
  </svg>
)

const SHOP: Record<Tab, Item[]> = {
  fleet: [
    {
      id: 'agent',
      name: 'Agent',
      desc: 'A piece on the parking. Its color says how it is: white ready, orange slow, red broken.',
      art: (
        <svg viewBox="-20 -32 40 38" aria-hidden className="size-full">
          <PieceSprite beacon="running" />
        </svg>
      ),
      dest: { to: '/deploy' },
      page: 'Deploy',
      place: 'bay',
    },
    {
      id: 'harness',
      name: 'Coding harness',
      desc: 'A local tool like Claude Code or Codex, on the dirt plot. Observed, never routed.',
      art: frame(2, 2, 46, <HarnessBall h={{ i: 0, j: 0, w: 2, d: 2, kind: 'claude' }} />),
      dest: { to: '/harnesses' },
      page: 'Coding harnesses',
      place: 'harness',
    },
  ],
  buildings: [
    {
      id: 'team',
      name: 'Team house',
      desc: 'A home for a team, with its budget battery beside it. Teams come from your organization.',
      art: frame(
        2.45,
        1.3,
        44,
        <TeamArt
          t={{
            i: 0,
            j: 0,
            color: '#a855f7',
            budget: { limit: 1000, spent: 620, projected: 900, hard: false },
            pace: 0.9,
          }}
        />,
      ),
    },
    {
      id: 'mcp',
      name: 'MCP gateway',
      desc: 'A grey house by the routers. Its flag says how its calls go; smoke means they fail.',
      art: frame(1.4, 1.4, 66, <McpHouse i={0} j={0} weather="ok" />),
      dest: { to: '/mcp' },
      page: 'MCP servers',
      place: 'mcp',
    },
    {
      id: 'tool',
      name: 'Tool',
      desc: 'A toolkit your agents call: a small box in front of the MCP gateways.',
      art: frame(0.75, 0.75, 16, <ToolBox i={0} j={0} brand="github" name="github" />),
      dest: { to: '/mcp' },
      page: 'MCP servers',
      place: 'tool',
    },
    {
      id: 'gate',
      name: 'LLM router',
      desc: 'A blue trough on stilts: the tokens pass under it on their way to the providers.',
      art: frame(0.56, 1.58, 58, <GateArt i={0} j={0} />),
      dest: { to: '/router' },
      page: 'LLM router',
      place: 'gate',
    },
    {
      id: 'mine',
      name: 'Provider',
      desc: 'A model provider: a tent past the routers, where the tokens go.',
      art: frame(
        2.42,
        2.42,
        54,
        <MineArt m={{ i: 0, j: 0, w: 2.42, color: '#c96f4a', mono: 'A' }} />,
      ),
      dest: { to: '/router' },
      page: 'LLM router',
      place: 'mine',
    },
  ],
  savings: [
    {
      id: 'orb',
      name: 'Opportunities',
      desc: 'An observatory that spots where to spend fewer tokens; its spark lights when there is one.',
      art: frame(1.53, 1.53, 60, <OpportunityArt i={0} j={0} on />),
      dest: { to: '/optimization' },
      page: 'Optimization',
    },
    {
      id: 'route',
      name: 'Route upgrade',
      desc: 'Adaptive cost and quality routing, fitted to a gate.',
      art: (
        <svg viewBox="-26 -26 52 52" aria-hidden className="size-full">
          <Rune x={0} y={0} on />
        </svg>
      ),
    },
  ],
  guardrails: [
    {
      id: 'wall',
      name: 'Wall',
      desc: 'A guardrail around a team: runs stop at its line.',
      art: <ShieldCheck aria-hidden className="size-12 text-muted-foreground" />,
    },
    {
      id: 'checkpoint',
      name: 'Checkpoint',
      desc: 'A guardrail on a token line: screens prompts and outputs as they pass.',
      art: <Milestone aria-hidden className="size-12 text-muted-foreground" />,
    },
    {
      id: 'ratelimit',
      name: 'Rate-limit tower',
      desc: 'Caps how deep, how wide and how long a flow can run.',
      art: <Gauge aria-hidden className="size-12 text-muted-foreground" />,
      dest: { to: '/settings' },
      page: 'Flow limits',
    },
  ],
}

export function Shop({
  open,
  onOpenChange,
  motion,
  canPlace,
  onPlace,
}: {
  open: boolean
  onOpenChange(o: boolean): void
  motion: boolean
  /** Whether a kind has an open, empty plot on this plan. */
  canPlace(kind: Kind): boolean
  /** Start placing `kind`; picking a plot then opens `dest`. */
  onPlace(kind: Kind, name: string, dest: Dest): void
}) {
  const [tab, setTab] = useState<Tab>('buildings')
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>{copy.shop.title}</DialogTitle>
          <DialogDescription>{copy.shop.sub}</DialogDescription>
        </DialogHeader>
        <Tabs value={tab} onValueChange={(t) => setTab(t as Tab)}>
          <TabsList>
            {(Object.keys(copy.shop.tabs) as Tab[]).map((t) => (
              <TabsTrigger key={t} value={t} className="px-3">
                {copy.shop.tabs[t]}
              </TabsTrigger>
            ))}
          </TabsList>
        </Tabs>
        <MotionOn.Provider value={motion}>
          <ul
            className="grid auto-rows-fr grid-cols-1 gap-3 sm:grid-cols-3 md:grid-cols-4"
            aria-label={copy.shop.tabs[tab]}
          >
            {SHOP[tab].map((it) => {
              const placeable = !!it.place && !!it.dest && canPlace(it.place)
              const body = (
                <>
                  <span className="grid h-28 w-full place-items-center rounded-t-lg bg-success/15">
                    {it.art}
                  </span>
                  {/* Room for the longest description (4 lines) and footer (2) on every card, so cards are one height in
                      every tab; a longer one grows its whole row (the grid's rows are equal). */}
                  <span className="flex flex-1 flex-col gap-1 px-3 pt-2 pb-3 text-left whitespace-normal">
                    <span className="text-sm font-semibold">{it.name}</span>
                    <span className="min-h-[4lh] text-xs font-normal text-muted-foreground">
                      {it.desc}
                    </span>
                    <span className="mt-auto min-h-[2lh] text-2xs font-medium text-muted-foreground">
                      {!it.dest
                        ? copy.shop.soon
                        : placeable
                          ? copy.shop.pick(it.page ?? '')
                          : it.place
                            ? `${copy.shop.full} · ${copy.shop.opens(it.page ?? '')}`
                            : copy.shop.opens(it.page ?? '')}
                    </span>
                  </span>
                </>
              )
              const card = 'h-full w-full flex-col items-stretch justify-start gap-0 p-0'
              return (
                <li key={it.id} className="h-full">
                  {it.dest && placeable && it.place ? (
                    <Button
                      variant="outline"
                      className={card}
                      onClick={() => {
                        onOpenChange(false)
                        if (it.place && it.dest) onPlace(it.place, it.name.toLowerCase(), it.dest)
                      }}
                    >
                      {body}
                    </Button>
                  ) : it.dest ? (
                    <Link
                      {...it.dest}
                      onClick={() => onOpenChange(false)}
                      {...asButton({ variant: 'outline', className: card })}
                    >
                      {body}
                    </Link>
                  ) : (
                    <div className="flex h-full flex-col rounded-lg border opacity-70">{body}</div>
                  )}
                </li>
              )
            })}
          </ul>
        </MotionOn.Provider>
      </DialogContent>
    </Dialog>
  )
}
