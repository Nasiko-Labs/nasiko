/** Every user-facing string of the Eval Universe page. */
export const copy = {
  title: 'Eval Universe',
  description:
    'Every code review task you have imported and every model attempt against it, plotted on one flat, shared precision/recall plane, colored by model. Drag to rotate, scroll to zoom, hover a point for its details — or use the task filter to isolate one PR.',
  loading: 'Loading your evals…',
  failed: "Couldn't load your evals",
  empty: 'Import a pull request in Code review evals to see it here.',
  legend: {
    title: 'Reading the scene',
    axes:
      'X = recall, Y = precision, both 0 to 1 on one shared scale — every attempt from every task plots on the same two axes, so models are directly comparable. Ungraded, pending, or failed attempts sit just off the corner, never at a fabricated (0, 0).',
    color: 'Color: model identity (same model = same color across every task). Gray = ungraded, pending, or failed.',
    size: 'Size: total tokens used (log-scaled) — not cost, since several models here are free.',
    labels: 'Hover a point to see its task, model name, and full stats. Use the Task filter to isolate one PR.',
    modelsTitle: 'Models',
  },
  filters: {
    title: 'Filter',
    task: 'Task',
    provider: 'Provider',
    model: 'Model',
    gradedOnly: 'Graded only',
  },
  tooltip: {
    status: 'Status',
    precision: 'Precision',
    recall: 'Recall',
    cost: 'Cost',
    tokens: 'Tokens',
    latency: 'Latency',
    ungraded: 'Ungraded preview',
  },
}
