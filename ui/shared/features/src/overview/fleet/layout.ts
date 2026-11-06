/**
 * Where everything sits on the Map view's board, in board units (the viewport scales them; 1 unit is 1 CSS px at
 * 100%). Five columns, left to right, the way the spend flows (Ans's wireframe, 2026-11-04): Org (every team against
 * its budget); the agents, the local harnesses and the workflows, stacked; the LLM routers over the connectors and
 * tools; the providers and their models; TokenOps (total spend, then what was saved and the month's budget under it).
 *
 * A column's cards stack (FleetBoard's `Stack`): each one as tall as what it holds, GAP from the next, the same gap in every column
 * (Ans, 2026-11-04: "reduce the vertical gap … keep them all same"). The heights below are only what the canvas pans to
 * before the board is measured; once it is, every card's own box (its `card:<id>` anchor) wins.
 *
 * The token lines are drawn under the cards: each one a curve across the gutter between two neighbouring columns
 * (lines.ts), so it never crosses a card's content.
 */
export const BOARD = { width: 1868, height: 1080 }

export interface Box {
  left: number
  top: number
  width: number
  height: number
}

/** Between two cards in one column. */
export const GAP = 12

/** The section headings' row, and where the cards start. */
export const TITLE_TOP = 100
const TOP = 144

/**
 * Proximity groups the columns (Ans, 2026-11-05: "all feels like a same group"): the three Fleet Runtime columns sit a
 * narrow INNER gutter apart, the sections (Org, Fleet Runtime, TokenOps) a wide SECTION gutter apart.
 */
const INNER = 44
const SECTION = 128
const ORG_LEFT = 32
const TEAMS_W = 250
const FLEET = ORG_LEFT + TEAMS_W + SECTION
const FLEET_W = 360
const ROUTE = FLEET + FLEET_W + INNER
const ROUTE_W = 250
const MODELS = ROUTE + ROUTE_W + INNER
const MODELS_W = 300
const TOKENOPS = MODELS + MODELS_W + SECTION
const TOKENOPS_W = 300

const AGENTS_H = 330
const HARNESS_H = 220
const WORKFLOWS_H = 280
const ROUTERS_H = 250
const BILL_H = 320
const SAVED_H = 160

const below = (b: Box) => b.top + b.height + GAP

const agents = { left: FLEET, top: TOP, width: FLEET_W, height: AGENTS_H }
const harness = { left: FLEET, top: below(agents), width: FLEET_W, height: HARNESS_H }
const routers = { left: ROUTE, top: TOP, width: ROUTE_W, height: ROUTERS_H }
const bill = { left: TOKENOPS, top: TOP, width: TOKENOPS_W, height: BILL_H }
const saved = { left: TOKENOPS, top: below(bill), width: TOKENOPS_W, height: SAVED_H }

export const CARDS = {
  teams: { left: ORG_LEFT, top: TOP, width: TEAMS_W, height: 560 },
  agents,
  harness,
  workflows: { left: FLEET, top: below(harness), width: FLEET_W, height: WORKFLOWS_H },
  routers,
  tools: { left: ROUTE, top: below(routers), width: ROUTE_W, height: 200 },
  providers: { left: MODELS, top: TOP, width: MODELS_W, height: 470 },
  bill,
  saved,
  budget: { left: TOKENOPS, top: below(saved), width: TOKENOPS_W, height: 180 },
} satisfies Record<string, Box>

export type CardId = keyof typeof CARDS
export const isCardId = (x: string): x is CardId => x in CARDS

/** The three sections' headings, over their first column. */
export const SECTIONS = [
  { id: 'org', left: CARDS.teams.left },
  { id: 'fleet', left: CARDS.agents.left },
  { id: 'tokenops', left: CARDS.bill.left },
] as const

/**
 * Where the lines run: g0, the gutter left of the fleet, holds the workflows' brackets up into the agents, clear of the
 * spend and calls lines leaving the fleet's right edge (Ans, 2026-11-05).
 */
export const LANES = {
  g0: CARDS.agents.left,
}

/**
 * The Map's home view (Ans, 2026-11-05): Fleet Runtime and TokenOps fill the canvas's width from their headings down,
 * the rest below the fold; Org is a pan to the left. The canvas opens on it and Fit returns to it.
 */
export const HOME = {
  x0: CARDS.agents.left - 96,
  y0: TITLE_TOP - 40,
  width: CARDS.bill.left + CARDS.bill.width + 96 - (CARDS.agents.left - 96),
  height: BOARD.height,
  widthOnly: true,
}
