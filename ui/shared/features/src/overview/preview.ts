/**
 * The coding harness preview, in place (2026-11-06; design: the "Harness Preview Variations" canvas, D): Home plays from
 * a developer's first command to a month in, on sample numbers, for a workspace with no coding harness connected. Four
 * steps of PREVIEW_STEP_MS: Day 1 grows from the sessions the sign-in imported, Week 1 from Day 1's figures, Month 1
 * from Week 1's. Once it ends, Home comes back after PREVIEW_HOLD_MS unless the viewer stays.
 *
 * Pure but for the context at the foot: `previewFrame(elapsed)` is where the play stands, `previewTick` moves its clock,
 * and `sampleSummary` turns a frame into the harness summary Home's own Harnesses tile and activity card draw.
 */
import { createContext, useContext, type Dispatch, type SetStateAction } from 'react'
import type { HarnessLine, HarnessSummary } from './api'
import { PREVIEW_HOLD_MS, PREVIEW_STEP_MS } from './tuning'

export const PREVIEW_STEPS = 4
export const PREVIEW_MS = PREVIEW_STEPS * PREVIEW_STEP_MS

/** The harnesses the sample team uses (ids from harnesses/constants): Claude Code and Codex from Day 1 (the sign-in
 *  found both), OpenCode from Month 1. */
const HARNESSES = ['claude', 'codex', 'opencode'] as const
/** Each model and the harness that runs it. */
const MODELS = [
  ['claude-sonnet-4', 0],
  ['claude-opus-4', 0],
  ['gpt-5-codex', 1],
  ['gpt-5', 1],
] as const
const TOOLS = ['Bash', 'Edit', 'Read', 'Grep', 'Write'] as const
/** The share of tool calls that failed. */
const FAILED = 0.028

interface Stop {
  /** Estimated cost per harness, HARNESSES order. */
  cost: readonly number[]
  devs: number
  /** Subagent runs per harness. */
  subagents: readonly number[]
  calls: number
  /** Share of turns per model, MODELS order. */
  models: readonly number[]
  /** Share of calls per tool, TOOLS order. */
  tools: readonly number[]
}

/** The past sessions `nasiko connect` imported: where Day 1 starts. */
const IMPORTED: Stop = {
  cost: [2.18, 0, 0],
  devs: 1,
  subagents: [0, 0, 0],
  calls: 41,
  models: [100, 0, 0, 0],
  tools: [50, 30, 20, 0, 0],
}
const DAY_1: Stop = {
  cost: [18.4, 0, 0],
  devs: 1,
  subagents: [6, 0, 0],
  calls: 410,
  models: [100, 0, 0, 0],
  tools: [44, 30, 26, 0, 0],
}
const WEEK_1: Stop = {
  cost: [214.1, 27.65, 0],
  devs: 6,
  subagents: [128, 12, 0],
  calls: 9_800,
  models: [52, 30, 18, 0],
  tools: [40, 26, 19, 9, 6],
}
const MONTH_1: Stop = {
  cost: [986.79, 128.36, 18.33],
  devs: 62,
  subagents: [881, 126, 23],
  calls: 71_584,
  models: [37.4, 27.9, 23.3, 11.4],
  tools: [37.9, 24.1, 20.7, 10.4, 6.9],
}

export interface PreviewFrame {
  /** 0 Today (the command), 1 Day 1, 2 Week 1, 3 Month 1. */
  step: number
  /** How far through its step, 0 to 1. */
  progress: number
  done: boolean
  /** The day the figures stand at: 1 to 7 over Week 1, 7 to 30 over Month 1. */
  day: number
  cost: number[]
  devs: number
  subagents: number[]
  calls: number
  models: number[]
  tools: number[]
}

const clamp = (x: number) => Math.max(0, Math.min(1, x))
const easeOut = (x: number) => 1 - (1 - clamp(x)) ** 3

export function previewFrame(elapsed: number): PreviewFrame {
  const done = elapsed >= PREVIEW_MS
  const step = Math.min(PREVIEW_STEPS - 1, Math.floor(Math.max(0, elapsed) / PREVIEW_STEP_MS))
  const progress = done ? 1 : clamp((elapsed - step * PREVIEW_STEP_MS) / PREVIEW_STEP_MS)
  const g = easeOut(progress)
  const [from, to] =
    step === 3 ? [WEEK_1, MONTH_1] : step === 2 ? [DAY_1, WEEK_1] : [IMPORTED, DAY_1]
  const lerp = (a: number, b: number) => a + (b - a) * g
  const each = (a: readonly number[], b: readonly number[]) => a.map((x, i) => lerp(x, b[i] ?? 0))
  return {
    step,
    progress,
    done,
    day: step === 2 ? Math.round(1 + 6 * g) : step === 3 ? Math.round(7 + 23 * g) : 1,
    cost: each(from.cost, to.cost),
    devs: lerp(from.devs, to.devs),
    subagents: each(from.subagents, to.subagents),
    calls: lerp(from.calls, to.calls),
    models: each(from.models, to.models),
    tools: each(from.tools, to.tools),
  }
}

/** Where a stop starts. Under reduced motion nothing grows, so a stop shows whole: its last moment, and the last stop's
 *  is the end (the countdown then runs). */
export function stepStart(step: number, reduced: boolean): number {
  const i = Math.max(0, Math.min(PREVIEW_STEPS - 1, step))
  if (!reduced) return i * PREVIEW_STEP_MS
  return i === PREVIEW_STEPS - 1 ? PREVIEW_MS : (i + 1) * PREVIEW_STEP_MS - 1
}

/** How many of `n` lines have appeared at `progress`: the first at once, the last a little before the step ends. */
export const revealed = (progress: number, n: number) =>
  Math.min(n, Math.max(1, Math.ceil(progress * n * 1.1)))

export interface PreviewState {
  on: boolean
  elapsed: number
  playing: boolean
  /** How long the countdown back to Home has run. */
  hold: number
  /** The viewer chose Stay: no countdown. */
  stay: boolean
}

export const PREVIEW_OFF: PreviewState = {
  on: false,
  elapsed: 0,
  playing: false,
  hold: 0,
  stay: false,
}

/** The preview from a stop; under reduced motion it never plays, it only shows. */
export function previewAt(step: number, reduced: boolean): PreviewState {
  const elapsed = stepStart(step, reduced)
  return { on: true, elapsed, playing: !reduced && elapsed < PREVIEW_MS, hold: 0, stay: false }
}

/** `ms` later: the play runs on; once it has ended, the countdown, and at its end Home is back. Paused or stayed, nothing
 *  moves. */
export function previewTick(s: PreviewState, ms: number): PreviewState {
  if (!s.on) return s
  if (s.playing) {
    const elapsed = Math.min(PREVIEW_MS, s.elapsed + ms)
    return { ...s, elapsed, playing: elapsed < PREVIEW_MS }
  }
  if (s.stay || s.elapsed < PREVIEW_MS) return s
  const hold = s.hold + ms
  return hold >= PREVIEW_HOLD_MS ? PREVIEW_OFF : { ...s, hold }
}

/** The frame as Home's harness summary (`useHarnessSummary`'s shape), so its own tile and card draw it. */
export function sampleSummary(f: PreviewFrame): HarnessSummary {
  const lines = HARNESSES.slice(0, f.step === PREVIEW_STEPS - 1 ? 3 : 2).map(
    (id, i): HarnessLine => {
      const models = MODELS.flatMap(([name, h], m) =>
        h === i && (f.models[m] ?? 0) > 0 ? [{ name, turns: f.models[m] ?? 0 }] : [],
      )
      return {
        id,
        cost: f.cost[i] ?? 0,
        turns: models.reduce((n, m) => n + m.turns, 0),
        models,
        subagentRuns: Math.round(f.subagents[i] ?? 0),
        // ponytail: every tool call on Claude Code; per-harness tools only if the preview ever shows them apart.
        tools:
          i === 0
            ? TOOLS.flatMap((name, t) => {
                const calls = ((f.tools[t] ?? 0) / 100) * f.calls
                return calls > 0 ? [{ name, calls, failed: calls * FAILED }] : []
              })
            : null,
        unpriced: false,
      }
    },
  )
  return {
    isPending: false,
    error: null,
    ownOnly: false,
    notVisible: false,
    connected: lines.length,
    activeDevs: Math.round(f.devs),
    harnesses: lines,
    retry: () => undefined,
  }
}

export interface PreviewControl {
  state: PreviewState
  reduced: boolean
  set: Dispatch<SetStateAction<PreviewState>>
}

/** Provided by Home (HarnessPreview.tsx `PreviewProvider`); null anywhere else, where nothing plays. */
export const PreviewContext = createContext<PreviewControl | null>(null)

export const usePreview = () => useContext(PreviewContext)

/** Home's harness summary on sample numbers while the preview plays past Today; null otherwise. */
export function usePreviewSample(): HarnessSummary | null {
  const state = useContext(PreviewContext)?.state
  if (!state?.on) return null
  const f = previewFrame(state.elapsed)
  return f.step > 0 ? sampleSummary(f) : null
}
