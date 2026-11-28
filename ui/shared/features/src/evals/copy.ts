/** Every user-facing string of Evals. */
export const copy = {
  title: 'Evals',
  description:
    'Turn your Claude Code sessions, or SWE benchmarks, into evals, then compare models on the same tasks.',
  loading: 'Loading evals',
  /** Links to the code review evals page, a sibling of this stepper (not its own nav entry). */
  codeReviewLink: 'Code review (preview)',
  absent: 'Evals need a newer Open Runtime server',
  absentBody: "This server doesn't have evals yet.",
  failed: "Evals couldn't load",
  retry: 'Try again',
  empty: {
    title: 'No evals to show',
    body: "Claude Code isn't connected yet, so there are no sessions to build evals from. Connect it first.",
    action: 'Connect a coding harness',
  },
  /** The stepper (shared/ui/src/composites/stepper.tsx). The first step is Sessions, or Benchmarks with none. */
  steps: {
    label: 'Evals steps',
    sessions: 'Sessions',
    benchmarks: 'Benchmarks',
    dataset: 'Dataset',
    compare: 'Compare',
  },
  sessions: {
    title: 'Pick sessions',
    hint: 'Each one is linked to the commit it produced in its repository; that request, fix and tests become a task.',
    ticked: (n: number) => `${n} ticked`,
    all: 'Select all usable sessions',
    one: (title: string) => `Select ${title}`,
    cols: {
      title: 'Session',
      repo: 'Repository',
      started: 'Started',
      turns: 'Turns',
      cost: 'Est. cost',
    },
    /** Why a session can't be ticked. */
    ineligible: {
      no_chat_history: 'Synced without chat history',
      no_edits: 'Edited no files',
      no_repo: 'Repository unknown: sync it again',
      not_github: 'Repository not on GitHub',
      github_not_connected: 'Connect GitHub to use it',
      repo_not_accessible: "Your GitHub account can't read its repository",
      unsupported_harness: 'Not supported for this harness yet',
    } as Record<string, string>,
  },
  benchmarks: {
    title: 'Pick benchmarks',
    hint: "Claude Code has no sessions yet, so start from SWE benchmarks: their tasks become the dataset's tasks.",
    picked: (n: number) => `${n} picked`,
    tasks: (n: number) => `${n} ${n === 1 ? 'task' : 'tasks'}`,
    none: 'Claude Code has no synced sessions yet, and this server offers no benchmarks. Sync your sessions with chat history on (nasiko integration sync), then come back.',
  },
  github: {
    title: 'Connect GitHub to build evals',
    body: "Evals link each session to its commit in the session's repository, read through your GitHub account.",
    action: 'Connect GitHub',
    waiting: 'Waiting for GitHub…',
    blocked: 'Your browser blocked the GitHub window.',
    openIt: 'Open GitHub sign-in',
    failed: "GitHub sign-in couldn't start.",
  },
  notRunning: "This server doesn't run evals, so a dataset can't be built here.",
  noChatHistory:
    "This server keeps no chat history from coding sessions, which evals' tasks are built from.",
  generate: (n: number) => (n ? `Generate evals dataset (${n})` : 'Generate evals dataset'),
  regenerate: (n: number) => `Generate a new dataset (${n})`,
  generating: 'Generating…',
  goToDataset: 'Go to dataset',
  opening: 'Opening dataset…',
  generateFailed: "The dataset couldn't be generated.",
  dataset: {
    title: 'Pick tasks and models',
    hint: 'Every ticked task runs on every ticked model.',
    loadFailed: "The dataset couldn't load.",
    building: (done: number, total: number) =>
      total
        ? `Building and testing tasks: ${done} of ${total} done. This can take a while; you can leave and come back.`
        : 'Linking the sessions to their commits…',
    failed: (why: string | null | undefined) =>
      `The dataset couldn't be built${why ? `: ${why}` : '.'}`,
    outcomes: (tasks: number, sessions: number) =>
      `${tasks} of ${sessions} ${sessions === 1 ? 'session' : 'sessions'} became ${tasks === 1 ? 'a task' : 'tasks'}.`,
    noTasks:
      'None of these sessions could be linked to a single commit, so there is nothing to run.',
    tasks: 'Tasks',
    tasksHint:
      'Each task is a request from a session, its real fix and the tests that prove it. Only verified tasks run; approve the ones that need review, or one the reviewers rejected that you would keep.',
    approveShort: 'Approve',
    rejectShort: 'Reject',
    approve: (task: string) => `Approve ${task}`,
    reject: (task: string) => `Reject ${task}`,
    noModels: 'You have no LLM configs to run the tasks on.',
    addModel: 'Add one in the LLM router',
    benchmarkTasksHint: 'From the benchmarks you picked.',
    models: 'Models',
    modelsHint:
      'Your LLM configs. The ones the sessions used are ticked: the fair comparison. Add others to compare.',
    benchmarkModelsHint: 'Pick the models to run the tasks on.',
    used: 'Used',
    status: {
      candidate: 'Preparing',
      reproducible: 'Needs review',
      verified: 'Verified',
      rejected: 'Not usable',
    } as Record<string, string>,
    picked: (tasks: number, models: number) =>
      `${tasks} ${tasks === 1 ? 'task' : 'tasks'} × ${models} ${models === 1 ? 'model' : 'models'}`,
  },
  run: 'Run evals',
  running: 'Starting…',
  runFailed: "The evals couldn't run.",
  results: {
    title: 'Comparison',
    task: 'Task',
    original: 'Original',
    best: 'Best',
    bestOverall: 'Best overall',
    byModel: 'Overall by model',
    prevModels: 'Previous models',
    nextModels: 'Next models',
    scoreOf: (model: string) => `${model}'s overall score`,
    summary: 'Overall',
    score: (n: number) => `${Math.round(n)}/100`,
    noResult: 'No result',
    waiting: 'Running…',
    optimistic: 'Likely flattering: this model also built the task',
    progress: (done: number, total: number) =>
      `Replaying the tasks: ${done} of ${total} attempts done. Each can take up to 45 minutes; you can leave and come back.`,
    ended: (status: string) =>
      status === 'cancelled' ? 'This run was cancelled.' : 'This run failed.',
    loadFailed: "The run couldn't load.",
    caption:
      'Score out of 100 (the share of the failing tests fixed, 0 if it broke others), cost from the router and time per task. The best score in each row is marked. Original is the session that did the work.',
    stale: 'Your picks changed since this run. Run evals again to compare them.',
  },
  saved: {
    title: 'Saved evals',
    open: (models: string) => `Open the run of ${models}`,
    status: {
      queued: 'Queued',
      running: 'Running',
      finalizing: 'Finishing',
      done: 'Done',
      failed: 'Failed',
      cancelled: 'Cancelled',
    } as Record<string, string>,
  },
}
