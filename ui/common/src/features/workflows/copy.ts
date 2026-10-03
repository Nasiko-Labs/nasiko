/**
 * Every user-facing string on the workflow pages (plans/feat-workflows.md), worded as the React migration's
 * workflows, workflow, workflow-new and executions pages say it.
 */
import { ApiError, isTimeoutError } from '@/lib/api/client'
import type { DraftSort, RunAgeFilter, RunStatusFilter, WorkflowSort } from './search'

export type ListMode = 'deployed' | 'drafts'

export const SORT_LABEL = {
  deployed: {
    recent: 'All',
    success_rate: 'Success rate',
    token_usage: 'Token usage',
    execution_count: 'Execution count',
    health: 'Health',
  } satisfies Record<WorkflowSort, string>,
  drafts: {
    all: 'All',
    last_updated: 'Last updated',
    token_usage: 'Token usage',
  } satisfies Record<DraftSort, string>,
}

export const STATUS_FILTER_LABEL: Record<RunStatusFilter, string> = {
  all: 'All',
  attention: 'Needs attention',
  running: 'Running',
  success: 'Completed',
  failed: 'Failed',
}
export const AGE_FILTER_LABEL: Record<RunAgeFilter, string> = {
  any: 'All time',
  '1d': 'Last 24 hours',
  '7d': 'Last 7 days',
  '30d': 'Last 30 days',
}

export const HEALTH_LABEL = { healthy: 'Healthy', degraded: 'Degraded', unhealthy: 'Unhealthy' }

export const copy = {
  nav: { deployed: 'Deployed', drafts: 'Drafts', runs: 'Runs', label: 'Workflows' },
  list: {
    deployed: {
      title: 'Deployed workflows',
      search: 'Search deployed workflows',
      empty: 'Deploy your first workflow',
      emptyText:
        'Turn a tested workflow into a reusable pipeline that you can run whenever you need it.',
      other: 'View drafts',
      loading: 'Loading deployed workflows',
    },
    drafts: {
      title: 'Draft workflows',
      search: 'Search draft workflows',
      empty: 'No workflows saved yet',
      emptyText:
        'Create a multi-agent workflow, test how the steps work together, and refine it before deploying.',
      other: 'View deployed',
      loading: 'Loading draft workflows',
    },
  } satisfies Record<ListMode, unknown>,
  searchPlaceholder: 'Search',
  sortLabel: 'Sort workflows',
  create: 'Create workflow',
  loadFailed: "Couldn't load workflows",
  noMatch: 'No workflows match',
  noMatchText: 'Try a different search term.',
  clearSearch: 'Clear search',
  retry: 'Retry',
  draftsAbsent: 'Drafts need a newer OpenRuntime server',
  draftsAbsentText:
    'This server keeps deployed workflows only. Create a workflow and deploy it, or update OpenRuntime to save drafts.',
  actions: 'Workflow actions',
  open: 'Open workflow',
  runNow: 'Run now',
  delete: 'Delete workflow',
  deleted: 'Workflow deleted',
  deleteTitle: (name: string) => `Delete ${name || 'workflow'}?`,
  deleteText: (name: string) =>
    `“${name}” will be deleted. Its past runs are kept. This action can't be undone.`,
  cancel: 'Cancel',
  confirmDelete: 'Delete',
  details: 'Details',
  steps: (n: number) => `${n} ${n === 1 ? 'step' : 'steps'}`,
  runs: (n: number) => `${n} ${n === 1 ? 'run' : 'runs'}`,
  success: (pct: string) => `${pct} success`,
  tokens: (t: string) => `${t} tokens`,
  created: (d: string) => `Created ${d}`,
  updated: (d: string) => `Updated ${d}`,
  runFailed: (why: string) => `Run failed: ${why}`,
  deleteFailed: (why: string) => `Delete failed: ${why}`,

  // Create
  newTitle: 'Create workflow',
  newText: 'Connect multiple AI agents into a structured workflow to automate complex tasks.',
  back: 'Back to workflows',
  nameLabel: 'Workflow name',
  nameRequired: 'Enter workflow name',
  namePlaceholder: 'Name this workflow',
  descPlaceholderNew: 'Describe workflow...',
  descPlaceholderEdit: 'Describe what this workflow is for',
  saveDraft: 'Save as draft and test',
  deploy: 'Deploy',
  saveFailed: (why: string) => `Save failed: ${why}`,
  deployedTitle: 'Workflow deployed',
  deployedText: 'Your workflow has been saved and is ready for the next step.',
  findInLibrary: 'Find in library',
  runWorkflow: 'Run workflow',
  runDidntStart: (why: string) => `Saved, but the run didn't start: ${why}`,
  leaveTitle: 'Leave without saving?',
  leaveText:
    "Your changes haven't been saved. If you leave now, you'll lose the changes made since your last save.",
  keepEditing: 'Keep editing',
  discard: 'Discard workflow',
  saveDraftShort: 'Save draft',
  needsStep: 'A workflow needs at least one step with instructions.',

  // Planner
  descLabel: 'Workflow description',
  generate: 'Generate plan',
  regenerate: 'Regenerate plan',
  generating: 'Generating your workflow...',
  plan: {
    'no-key':
      "AI drafting isn't available — this server has no OpenAI API key configured. You can still add steps manually below.",
    noAgentsBefore: "You don't have any agents yet, so there's nothing to plan with.",
    noAgentsLink: 'Deploy an agent',
    noAgentsAfter: 'first, then draft steps.',
    planner:
      "OpenRuntime couldn't draft steps from that description — try rephrasing it, or add the steps manually below.",
    other: (why: string) => `Drafting failed: ${why}`,
  },
  replaceTitle: 'Replace your steps?',
  replaceText: 'A new plan replaces every step below, including the changes you made to them.',
  keepSteps: 'Keep my steps',
  replaceSteps: 'Replace steps',

  // Step editor
  noSteps: 'No steps yet',
  noStepsText: 'Add the first step, then tell it what to do and which agent should run it.',
  step: (n: number) => `Step ${n}`,
  reorder: (n: number) => `Reorder step ${n} — drag, or press the up and down arrow keys`,
  dragHint: 'Drag to reorder',
  removeStep: (n: number) => `Remove step ${n}`,
  instructions: (n: number) => `Instructions for step ${n}`,
  instructionsPlaceholder: 'Tell this agent what to do...',
  chooseAgent: 'Choose agent',
  agentFor: (n: number) => `Agent for step ${n}`,
  agentPlaceholder: 'Leave blank to auto-assign an agent when saved',
  suggested: 'Suggested',
  notDeployed: 'Not deployed',
  insertFirst: 'Insert step at the beginning',
  insertAfter: (n: number) => `Insert step after step ${n}`,
  addStep: 'Add step',
  moved: (to: number, of: number) => `Step moved to position ${to} of ${of}`,

  // Workflow page
  noneSelected: 'No workflow selected',
  notFound: 'Workflow not found',
  notFoundText: 'It may have been deleted.',
  backToWorkflows: 'Back to workflows',
  loadOneFailed: "Couldn't load this workflow",
  loadingWorkflow: 'Loading workflow',
  crumbDeployed: 'Workflows',
  crumbDrafts: 'Drafts',
  stepsTitle: 'Steps',
  assignedWhenSaved: 'Assigned when saved',
  noStepsLive: 'Hit Edit to add the first step.',
  outputGuidelines: 'Output guidelines',
  edit: 'Edit',
  run: 'Run',
  saveAndRun: 'Save & run',
  deployedToast: 'Workflow deployed',
  stepFailed: (at: 'Save' | 'Run' | 'Deploy', why: string) => `${at} failed: ${why}`,
  dangerZone: 'Danger zone',
  dangerLive: 'Remove this workflow from your deployed workflows.',
  dangerDraft: 'Remove this draft.',
  dangerAfter: 'Existing workflow runs will not be affected.',

  // Runs
  startingRun: 'Starting the run…',
  backToWorkflow: 'Back to workflow',
  execution: 'Execution',
  executionN: (n: number) => `Execution #${n}`,
  runDetails: 'Run details',
  started: (when: string) => `Started ${when}`,
  attempt: (n: number, of: number) => `attempt ${n}/${of}`,
  output: 'Output',
  prompt: 'Prompt',
  promptFor: (n: number) => `Prompt for step ${n}`,
  stepsLabel: 'Steps',
  stepTitle: (n: number, what: string) => `Step ${n} — ${what}`,
  unassigned: 'Unassigned',
  planning: 'Planning & final synthesis',
  runNotFound: 'Run not found',
  runNotFoundText: 'It may have been removed, or it belongs to someone else.',
  loadRunFailed: "Couldn't load this run",
  loadingRun: 'Loading run',
  loadingRuns: 'Loading workflow runs',
  runsTitle: 'Workflow runs',
  runsSearch: 'Search workflow runs',
  statusFilter: 'Filter by status',
  ageFilter: 'Filter by age',
  runsLoadFailed: "Couldn't load executions",
  noRuns: 'No workflow runs yet',
  noRunsText: 'Your workflow runs will appear here once you start executing a deployed workflow.',
  viewWorkflows: 'View workflows',
  noRunsMatch: 'No runs match these filters',
  noRunsMatchText: 'Try another search, status or age.',
  clearFilters: 'Clear filters',
  orphan: 'Workflow not found',
  openDraft: 'Open draft',
  rerun: 'Rerun',
  viewTrace: 'View trace',
  couldNotStart: 'Could not start the run.',
  runsList: 'Workflow runs',
  thisAgent: 'This agent',
}

/** The server's own reason (MAF errors carry it as `message`), else a plain fallback. */
export function reason(err: unknown, fallback = 'The request failed.'): string {
  if (isTimeoutError(err)) return 'The server took too long to answer.'
  if (err instanceof ApiError) return err.serverMessage ?? fallback
  return fallback
}
