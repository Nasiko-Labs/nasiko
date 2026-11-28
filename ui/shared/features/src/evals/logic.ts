/** The comparison table's maths: a row per task, a cell per model, the best score per row, and a footer per model. */
import type { EvalDataset, EvalModel, EvalResult, EvalTask } from './types'

/** One cell: a graded attempt, one still going, or one that never produced a grade. */
export type Cell =
  | {
      kind: 'graded'
      score: number
      /** The router's cost; null until the run is finalized, or when the model has no price. */
      cost: number | null
      latency: number | null
      /** The row's highest score (every model that ties it). */
      best: boolean
      /** The model also rewrote or screened the task: its score is optimistic. */
      builder: boolean
      failure: string | null
    }
  | { kind: 'waiting' }
  | { kind: 'failed' }

export interface ModelTotals {
  /** Mean score over the tasks it was graded on. */
  score: number
  /** Total of the known costs; null when none is known yet. */
  cost: number | null
  /** Median latency of the graded attempts. */
  latency: number | null
}

export interface Comparison {
  rows: { taskId: string; cells: (Cell | null)[] }[]
  /** Per model, in column order; null when it has no graded attempt. */
  footer: (ModelTotals | null)[]
}

function median(xs: number[]): number | null {
  if (!xs.length) return null
  const s = [...xs].sort((a, b) => a - b)
  const mid = Math.floor(s.length / 2)
  const hi = s[mid] ?? 0
  return s.length % 2 ? hi : ((s[mid - 1] ?? 0) + hi) / 2
}

function cellOf(r: EvalResult | undefined): Cell | null {
  if (!r) return null
  if (r.status === 'pending' || r.status === 'running') return { kind: 'waiting' }
  if (r.status !== 'done' || r.score == null) return { kind: 'failed' }
  return {
    kind: 'graded',
    score: r.score,
    cost: r.cost_usd,
    latency: r.latency_ms,
    best: false,
    builder: r.builder,
    failure: r.failure,
  }
}

/** The task × model table from a run's first attempts. */
export function comparison(
  taskIds: readonly string[],
  modelIds: readonly string[],
  results: readonly EvalResult[],
): Comparison {
  const first = results.filter((r) => (r.attempt ?? 1) === 1)
  const at = new Map(first.map((r) => [`${r.task_id}\n${r.model_id}`, r]))
  const rows = taskIds.map((taskId) => {
    const found = modelIds.map((m) => cellOf(at.get(`${taskId}\n${m}`)))
    const scores = found.flatMap((c) => (c?.kind === 'graded' ? [c.score] : []))
    const top = scores.length ? Math.max(...scores) : null
    return {
      taskId,
      cells: found.map((c) => (c?.kind === 'graded' ? { ...c, best: c.score === top } : c)),
    }
  })
  const footer = modelIds.map((_, i) => {
    const cells = rows.flatMap((row) => {
      const c = row.cells[i]
      return c?.kind === 'graded' ? [c] : []
    })
    if (!cells.length) return null
    const costs = cells.flatMap((c) => (c.cost == null ? [] : [c.cost]))
    return {
      score: cells.reduce((n, c) => n + c.score, 0) / cells.length,
      cost: costs.length ? costs.reduce((n, c) => n + c, 0) : null,
      latency: median(cells.flatMap((c) => (c.latency == null ? [] : [c.latency]))),
    }
  })
  return { rows, footer }
}

/** The tasks a run can take: only verified ones. */
export const runnable = (tasks: readonly EvalTask[]) => tasks.filter((t) => t.status === 'verified')

/** The gate kept the task and no reviewer model judged it: a person approves or rejects it. */
export const needsReview = (t: EvalTask) => t.status === 'reproducible' && t.stage === 'review'

/** A person can approve a task waiting for review, and one the reviewers rejected after the gate kept it. */
export const canApprove = (t: EvalTask) =>
  needsReview(t) || (t.status === 'rejected' && t.reviewable === true)

/** What a dataset starts with ticked: every runnable task, and the models the sessions used. */
export function defaultPicks(dataset: EvalDataset): { tasks: string[]; models: string[] } {
  return {
    tasks: runnable(dataset.tasks).map((t) => t.id),
    models: dataset.models.filter((m: EvalModel) => m.used).map((m) => m.id),
  }
}

/** The dataset is still being built. */
export const building = (d: EvalDataset) => d.status === 'harvesting' || d.status === 'preparing'
