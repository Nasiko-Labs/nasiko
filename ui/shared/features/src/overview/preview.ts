/**
 * The coding harness preview (2026-11-06; design: the "Harness Preview Variations" canvas, C with B's timeline folded
 * in): Home from a developer's first command to a month in, on sample numbers, for a workspace with no coding harness
 * connected yet. Four steps of PREVIEW_STEP_MS each; Week 1 grows from Day 1's figures and Month 1 from Week 1's.
 * Pure: the dialog draws `previewFrame(elapsed)`.
 */
import { PREVIEW_STEP_MS } from './tuning'

export const PREVIEW_STEPS = 4
export const PREVIEW_MS = PREVIEW_STEPS * PREVIEW_STEP_MS

/** The harnesses the sample team uses, in the order their bars and rows draw (ids from harnesses/constants). */
export const PREVIEW_HARNESSES = ['claude', 'codex', 'opencode'] as const
export const PREVIEW_MODELS = ['claude-sonnet-4', 'claude-opus-4', 'gpt-5-codex'] as const
export const PREVIEW_TOOLS = ['Bash', 'Edit', 'Read'] as const

/** Day 1's feed: harness index, task, turns, cost. */
export const PREVIEW_SESSIONS = [
  [0, 'Refactor the payment retry logic', 41, 2.18],
  [0, 'Add tests for the invoice parser', 18, 0.74],
  [1, 'Fix the flaky CI job', 9, 0.31],
  [0, 'Migrate the settings form', 27, 1.12],
  [2, 'Explain the build worker', 6, 0.08],
  [1, 'Bump the SDK and fix type errors', 14, 0.46],
  [0, 'Review the rate limiter change', 22, 0.97],
] as const

/** The Agent runs tile's bars, as shares of the tallest. */
const BARS = [0.3, 0.75, 0.48, 0.42, 0.85, 1, 0.8, 0.55]
/** The Tools tile scales its bars to the busiest tool's best share. */
const TOOL_SCALE = 44

interface Stop {
  runs: number
  /** Estimated cost per harness, PREVIEW_HARNESSES order. */
  cost: readonly number[]
  devs: number
  /** Subagent runs per harness. */
  subagents: readonly number[]
  calls: number
  /** Share of turns per model, PREVIEW_MODELS order. */
  models: readonly number[]
  /** Share of calls per tool, PREVIEW_TOOLS order. */
  tools: readonly number[]
}

const DAY_1: Stop = {
  runs: 84,
  cost: [18.4, 0, 0],
  devs: 1,
  subagents: [6, 0, 0],
  calls: 410,
  models: [100, 0, 0],
  tools: [44, 30, 26],
}
const WEEK_1: Stop = {
  runs: 712,
  cost: [214.1, 27.65, 0],
  devs: 6,
  subagents: [128, 12, 0],
  calls: 9_800,
  models: [52, 30, 18],
  tools: [40, 26, 19],
}
const MONTH_1: Stop = {
  runs: 2_555,
  cost: [986.79, 128.36, 18.33],
  devs: 62,
  subagents: [881, 126, 23],
  calls: 71_584,
  models: [37.4, 27.9, 23.3],
  tools: [37.9, 24.1, 20.7],
}

export interface PreviewFrame {
  /** 0 Today (the command), 1 Day 1 (sessions arrive), 2 Week 1 (two tiles), 3 Month 1 (and three more). */
  step: number
  /** How far through its step, 0 to 1. */
  progress: number
  done: boolean
  /** The day the tiles stand at: 1 to 7 over Week 1, 7 to 30 over Month 1. */
  day: number
  runs: number
  runsPerDay: number
  cost: number[]
  devs: number
  subagents: number[]
  calls: number
  models: number[]
  tools: { share: number; width: number }[]
  /** Each bar's height, as a share of the tile's chart. */
  bars: number[]
}

const clamp = (x: number) => Math.max(0, Math.min(1, x))
const easeOut = (x: number) => 1 - (1 - clamp(x)) ** 3

export function previewFrame(elapsed: number): PreviewFrame {
  const done = elapsed >= PREVIEW_MS
  const step = Math.min(PREVIEW_STEPS - 1, Math.floor(Math.max(0, elapsed) / PREVIEW_STEP_MS))
  const progress = done ? 1 : clamp((elapsed - step * PREVIEW_STEP_MS) / PREVIEW_STEP_MS)
  const g = easeOut(progress)
  const [from, to] = step === 3 ? [WEEK_1, MONTH_1] : [DAY_1, WEEK_1]
  const lerp = (a: number, b: number) => a + (b - a) * g
  const each = (a: readonly number[], b: readonly number[]) => a.map((x, i) => lerp(x, b[i] ?? 0))
  const day = step === 2 ? Math.round(1 + 6 * g) : step === 3 ? Math.round(7 + 23 * g) : 1
  const runs = lerp(from.runs, to.runs)
  const reach = step === 3 ? 1 : 0.35 + 0.65 * g
  return {
    step,
    progress,
    done,
    day,
    runs,
    runsPerDay: runs / day,
    cost: each(from.cost, to.cost),
    devs: lerp(from.devs, to.devs),
    subagents: each(from.subagents, to.subagents),
    calls: lerp(from.calls, to.calls),
    models: each(from.models, to.models),
    tools: each(from.tools, to.tools).map((share) => ({
      share,
      width: (share / TOOL_SCALE) * 100,
    })),
    bars: BARS.map((h) => h * reach),
  }
}

/** Where a jump to `step` lands: its start, or its end under reduced motion, where nothing grows. Past the last step is
 *  the end. */
export function stepStart(step: number, reduced: boolean): number {
  if (step >= PREVIEW_STEPS) return PREVIEW_MS
  return Math.max(0, step) * PREVIEW_STEP_MS + (reduced ? PREVIEW_STEP_MS - 1 : 0)
}

/** How many of `n` lines have appeared at `progress`: the first at once, the last a little before the step ends. */
export const revealed = (progress: number, n: number) =>
  Math.min(n, Math.max(1, Math.ceil(progress * n * 1.1)))
