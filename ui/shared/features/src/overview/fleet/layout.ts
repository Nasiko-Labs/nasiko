/**
 * Where everything sits on the Map view's board, in board units (the viewport scales them; 1 unit is 1 CSS px at
 * 100%). Three sections, left to right (Ans's wireframe, 2026-10-17, second round): Org (every team against its
 * budget, the full height), Fleet Runtime (agents and workflows on top; under the agents the LLM routers, a small box,
 * over the providers and their models; under the workflows the connectors and tools; the local harnesses across the
 * bottom) and TokenOps (the bill, the full height, on the left; the savings globe over the opportunities and the
 * budget on the right). Inside a section every card is GAP from its neighbours; sections are GUTTER apart.
 *
 * The token lines are drawn under the cards and run in the gaps: the gutter beside the teams (g0), the gap between the
 * agents and the workflows (g1), the street under the top row (s1), the band left of the routers, the street between
 * the routers and the providers (s3) and the street over the harnesses (s4), so a line enters a card at its edge and
 * never crosses a card's content (lines.ts).
 */
export const BOARD = { width: 1748, height: 912 }

export interface Box {
  left: number
  top: number
  width: number
  height: number
}

/** Between two cards in one section, and between two sections. */
export const GAP = 20
const GUTTER = 72

/** The section headings' row, and where the cards start and end. */
export const TITLE_TOP = 100
const TOP = 144
/** The Fleet Runtime's top row ends here; the routers and the tools start a GAP under it (the street s1). */
const ROW = 464
const LOW = ROW + GAP
/** Where the Org, the providers, the tools and TokenOps end; the harnesses start a GAP under it (the street s4). */
const BOTTOM = 720
const HARNESS_TOP = BOTTOM + GAP
const HARNESS_H = 132

const ORG_LEFT = 32
const TEAMS_W = 250
const FLEET = ORG_LEFT + TEAMS_W + GUTTER
const AGENTS_W = 420
const WORKFLOWS_W = 250
const FLEET_RIGHT = FLEET + AGENTS_W + GAP + WORKFLOWS_W
const TOKENOPS = FLEET_RIGHT + GUTTER
const BILL_W = 300
const SIDE = TOKENOPS + BILL_W + GAP
const SIDE_W = 280

/** The routers' box: one row of tiles. */
const ROUTERS_H = 56
const SAVED_H = 184
const OPP_TOP = TOP + SAVED_H + GAP
const BUDGET_H = 180

export const CARDS = {
  teams: { left: ORG_LEFT, top: TOP, width: TEAMS_W, height: BOTTOM - TOP },
  agents: { left: FLEET, top: TOP, width: AGENTS_W, height: ROW - TOP },
  workflows: { left: FLEET + AGENTS_W + GAP, top: TOP, width: WORKFLOWS_W, height: ROW - TOP },
  // A GAP in from the providers' left edge: the direct lines (no router) run down that band.
  routers: { left: FLEET + GAP, top: LOW, width: AGENTS_W - GAP, height: ROUTERS_H },
  providers: {
    left: FLEET,
    top: LOW + ROUTERS_H + GAP,
    width: AGENTS_W,
    height: BOTTOM - (LOW + ROUTERS_H + GAP),
  },
  tools: { left: FLEET + AGENTS_W + GAP, top: LOW, width: WORKFLOWS_W, height: BOTTOM - LOW },
  harness: { left: FLEET, top: HARNESS_TOP, width: FLEET_RIGHT - FLEET, height: HARNESS_H },
  bill: { left: TOKENOPS, top: TOP, width: BILL_W, height: BOTTOM - TOP },
  saved: { left: SIDE + (SIDE_W - SAVED_H) / 2, top: TOP, width: SAVED_H, height: SAVED_H },
  opportunities: {
    left: SIDE,
    top: OPP_TOP,
    width: SIDE_W,
    height: BOTTOM - BUDGET_H - GAP - OPP_TOP,
  },
  budget: { left: SIDE, top: BOTTOM - BUDGET_H, width: SIDE_W, height: BUDGET_H },
} satisfies Record<string, Box>

export type CardId = keyof typeof CARDS
export const isCardId = (x: string): x is CardId => x in CARDS

/** The three sections' headings, over their first column. */
export const SECTIONS = [
  { id: 'org', left: CARDS.teams.left },
  { id: 'fleet', left: CARDS.agents.left },
  { id: 'tokenops', left: CARDS.bill.left },
] as const

/** Where the lines run: x of each gutter's left edge and y of each street's top. */
export const LANES = {
  /** Teams → agents and harnesses (one lane per team). */
  g0: CARDS.teams.left + CARDS.teams.width,
  /** Between the agents and the workflows: a lane per router, per tool and per workflow. */
  g1: CARDS.agents.left + CARDS.agents.width,
  /** Under the top row: agents across to the routers and the tools, the direct lines to the band. */
  s1: ROW,
  /** The band left of the routers: the direct lines down to the providers. */
  band: CARDS.providers.left,
  /** Between the routers and the providers: every line into a model from above. */
  s3: CARDS.routers.top + CARDS.routers.height,
  /** Over the harnesses: their lines up to the models and tools, and the month's payments to the providers. */
  s4: BOTTOM,
  /** Between the bill and the budget: the payments' way into the budget. */
  g3: CARDS.bill.left + CARDS.bill.width,
}
