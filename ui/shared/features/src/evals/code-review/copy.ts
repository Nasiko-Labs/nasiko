/** Every user-facing string of code review evals. */
export const copy = {
  title: 'Code review evals',
  description:
    'Import a pull request, add validated reference findings if you have them, and compare how different models review the same diff.',
  universeLink: 'Eval Universe (preview)',
  import: {
    title: 'Import a pull request',
    owner: 'Owner',
    repo: 'Repo',
    prNumber: 'PR number',
    action: 'Import',
    importing: 'Importing…',
    failed: "Couldn't import that pull request",
  },
  task: {
    files: (n: number) => `${n} touched file${n === 1 ? '' : 's'}`,
    graded: 'Graded',
    ungraded: 'Ungraded preview',
    noReference: 'No reference findings yet',
    noReferenceBody:
      'A run on this task shows what each model found, but not a trustworthy score — add validated reference findings below first.',
  },
  findings: {
    label: 'Reference findings (JSON array)',
    placeholder:
      '[{"file": "src/a.rs", "line": 42, "severity": "high", "category": "bug", "description": "..."}]',
    action: 'Save reference findings',
    invalidJson: 'That is not valid JSON',
    saveFailed: "Couldn't save those findings — check they match the expected shape",
  },
  models: {
    label: 'Models to compare',
    action: 'Run review',
    running: 'Starting…',
  },
  results: {
    loading: 'Loading the run…',
    failed: "Couldn't load the run",
    title: 'Results',
    graded: 'Graded against reference findings',
    ungraded: 'Ungraded preview — not a trustworthy benchmark result',
    preview: '—',
    cols: {
      model: 'Model',
      status: 'Status',
      precision: 'Precision',
      recall: 'Recall',
      cost: 'Cost',
      latency: 'Latency',
      priceSource: 'Price source',
    },
    taskCount: 'Based on 1 task.',
  },
}
