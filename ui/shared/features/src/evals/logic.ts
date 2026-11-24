/** The comparison table's maths: a row per task, a cell per model, the best score per row, and a footer per model. */
import type { EvalResult } from './types'

export interface Cell {
  score: number
  cost: number
  latency: number
  /** The row's highest score (every model that ties it). */
  best: boolean
}

export interface ModelTotals {
  /** Mean score over the tasks it answered. */
  score: number
  cost: number
  /** Median latency. */
  latency: number
}

export interface Comparison {
  rows: { taskId: string; cells: (Cell | null)[] }[]
  /** Per model, in column order; null when it has no results. */
  footer: (ModelTotals | null)[]
}

function median(xs: number[]): number {
  const s = [...xs].sort((a, b) => a - b)
  const mid = Math.floor(s.length / 2)
  const hi = s[mid] ?? 0
  return s.length % 2 ? hi : ((s[mid - 1] ?? 0) + hi) / 2
}

export function comparison(
  taskIds: readonly string[],
  modelIds: readonly string[],
  results: readonly EvalResult[],
): Comparison {
  const at = new Map(results.map((r) => [`${r.task_id}\n${r.model_id}`, r]))
  const rows = taskIds.map((taskId) => {
    const found = modelIds.map((m) => at.get(`${taskId}\n${m}`) ?? null)
    const top = Math.max(...found.map((r) => r?.score ?? -Infinity))
    return {
      taskId,
      cells: found.map((r) =>
        r
          ? { score: r.score, cost: r.cost_usd, latency: r.latency_ms, best: r.score === top }
          : null,
      ),
    }
  })
  const footer = modelIds.map((_, i) => {
    const cells = rows.flatMap((row) => (row.cells[i] ? [row.cells[i]] : []))
    if (!cells.length) return null
    return {
      score: cells.reduce((n, c) => n + c.score, 0) / cells.length,
      cost: cells.reduce((n, c) => n + c.cost, 0),
      latency: median(cells.map((c) => c.latency)),
    }
  })
  return { rows, footer }
}
