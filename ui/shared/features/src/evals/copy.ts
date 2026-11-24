/** Every user-facing string of Evals. */
export const copy = {
  title: 'Evals',
  description:
    "Turn your coding harnesses' sessions into evals, then compare models on the same tasks.",
  loading: 'Loading evals',
  absent: 'Evals need a newer Open Runtime server',
  absentBody: "This server doesn't have evals yet.",
  failed: "Evals couldn't load",
  retry: 'Try again',
  empty: {
    title: 'No evals to show',
    body: 'No coding harness is connected yet, so there are no sessions to build evals from. Connect one first.',
    action: 'Connect a coding harness',
  },
  pickHarness: 'Pick a harness to build evals from its sessions.',
  sessionCount: (n: number) => `${n} ${n === 1 ? 'session' : 'sessions'}`,
  lastActive: (when: string) => `Last active ${when}`,
  notFound: "This harness isn't connected",
  notFoundBody: 'It may have been disconnected. Pick another harness.',
  back: 'Back to Evals',
  sessions: {
    title: 'Sessions',
    hint: 'Tick the sessions to build the dataset from. Each one is linked to the commit it produced in its repository.',
    all: 'Select all usable sessions',
    one: (title: string) => `Select ${title}`,
    cols: {
      title: 'Session',
      repo: 'Repository',
      started: 'Started',
      turns: 'Turns',
      cost: 'Est. cost',
    },
    none: 'This harness has no sessions yet.',
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
  generateFailed: "The dataset couldn't be generated.",
  dataset: {
    title: 'Dataset',
    loadFailed: "The dataset couldn't load.",
    building: (done: number, total: number) =>
      total
        ? `Building and testing tasks: ${done} of ${total} done. This can take a while; you can leave and come back.`
        : 'Linking the sessions to their commits…',
    failed: (why: string | null) => `The dataset couldn't be built${why ? `: ${why}` : '.'}`,
    outcomes: (tasks: number, sessions: number) =>
      `${tasks} of ${sessions} ${sessions === 1 ? 'session' : 'sessions'} became ${tasks === 1 ? 'a task' : 'tasks'}.`,
    noTasks:
      'None of these sessions could be linked to a single commit, so there is nothing to run.',
    tasks: 'Tasks',
    tasksHint:
      'Each task is a request from a session, its real fix and the tests that prove it. Only verified tasks run.',
    models: 'Models',
    modelsHint:
      'Your LLM configs. The ones the sessions used are ticked: the fair comparison. Add others to compare.',
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
    summary: 'Overall',
    score: (n: number) => `${Math.round(n)}/100`,
    noResult: 'No result',
    waiting: 'Running…',
    optimistic: 'Optimistic: this model also built the task',
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
    none: 'No evals run yet.',
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
