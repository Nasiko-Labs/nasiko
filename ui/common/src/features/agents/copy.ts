/**
 * Every user-facing string on the agent pages (plan §6.4, DX). Retired wording ("Shared with
 * you", "last deployed", a "Registered" badge) must not come back; logic.test.ts checks. Status
 * labels and hints live in the status matrix (status.ts), the single status source.
 */
import { ApiError } from '@/lib/api/client'
import { SERVER_TEXT_MAX } from './tuning'

export const copy = {
  catalogTitle: 'Agents',
  mineTitle: 'Your agents',
  // The module's sub-nav (components/bits AgentsNav): Deploy and Builds moved here from the sidebar.
  sections: { label: 'Agents', all: 'All agents', mine: 'Your agents', builds: 'Builds' },
  searchLabel: 'Search agents',
  searchPlaceholder: 'Search by name, description, tag or skill',
  yours: 'Yours',
  availableToYou: 'Available to you',
  publicChip: 'Public',
  harnessChip: 'Coding harness',
  showHarnesses: (n: number) => `Show coding harnesses (${n})`,
  hideHarnesses: 'Hide coding harnesses',
  harnessUsage: 'See usage on Harnesses',
  excludesHarnesses: (n: number) => `Excludes ${n} coding harness${n === 1 ? '' : 'es'}`,
  noResults: 'No agents match this search',
  noResultsHint: 'Try another name, tag or skill, or clear the search and filters.',
  clearSearch: 'Clear search',
  /** The Overview's Fleet health filter (overview design 15A). */
  healthChip: (rating: string) => `Health: ${rating}`,
  clearHealth: (rating: string) => `Remove the Health: ${rating} filter`,
  healthReason: (reason: string) => `Why: ${reason}`,
  healthCostFailed:
    "Couldn't load cost data: cost ratings read Unknown, so this list may be incomplete.",
  loadingMore: 'Loading more…',
  loadingAgents: 'Loading agents',
  loadingAgent: 'Loading agent',
  partialLoad: "Couldn't load all agents.",
  noAgentsCatalog: 'No agents yet',
  noAgentsCatalogHint: 'Deploy one here, or from a terminal with the CLI below.',
  noAgentsMine: "You haven't deployed an agent yet",
  noAgentsMineHint: 'Agents you deploy show up here with their status and usage.',
  firstRunIntro: 'Deploy your first agent from a terminal:',
  firstRunSteps: [
    'Connect the CLI to this server',
    'Create an agent project',
    'Build and deploy it',
  ],
  deployHint: 'Deploy with',
  needsAttention: 'Needs attention',
  tabAll: 'All',
  tabHarnesses: 'Harnesses',
  colName: 'Name',
  colStatus: 'Status',
  colVersion: 'Version',
  colUpdated: 'Updated',
  colTurns: '24h turns',
  colCost: '24h est. cost',
  turnsTip: 'Turns: agent calls recorded in traces.',
  turnsSuffix: (n: number | undefined) => (n === 1 ? 'turn' : 'turns'),
  usageUnavailable: '24h usage unavailable.',
  owner: 'Owner',
  actions: 'Actions',
  actionsFor: (name: string) => `Actions for ${name}`,
  moreActionsFor: (name: string) => `More actions for ${name}`,
  whyAttention: (name: string) => `Why ${name} needs attention`,
  crashNoReason: 'The server recorded no reason; open the agent for recent error lines.',
  noAgentsInTab: (tab: string) => `No agents in ${tab}`,
  noAgentsInTabHint: 'None of your agents are in this tab right now.',
  showAllAgents: 'Show all your agents',
  agentsCount: (n: number) => `${n} agent${n === 1 ? '' : 's'}`,
  dismiss: 'Dismiss',
  deletedNotice: (name: string) => `Deleted ${name}.`,
  deletedWithErrors: (name: string, stopped: number) =>
    `Deleted ${name}, but the runtime reported errors (${stopped} container${stopped === 1 ? '' : 's'} stopped):`,

  notFound: 'Agent not found or not visible.',
  notFoundFix: 'Check the link or ask the owner.',
  multipleNamed: (name: string) => `Multiple agents are named ${name}`,
  multipleNamedFix: 'Pick one:',
  ownedBy: (owner: string) =>
    `Owned by ${owner}. Only the owner or a superuser can change access or settings.`,
  you: 'You',

  tabOverview: 'Overview',
  tabActivity: 'Activity',
  tabVersions: 'Versions',
  tabBuilds: 'Builds',
  tabMcp: 'MCP',
  tabAccess: 'Access',
  tabSettings: 'Settings',

  crashTitle: 'This agent crashed',
  crashOss: 'The runtime reports this agent as crashed. Recent error lines:',
  failedTitle: 'This agent failed to deploy',
  failedOss: 'The runtime reports this agent as failed. Recent error lines:',
  crashNoLines: 'No error lines in the latest logs.',
  viewAllLogs: 'View all logs',
  activeVersion: 'Active version',
  deployed: 'Deployed',
  image: 'Image',
  serviceUrl: 'Service URL',
  notDeployed: 'Not deployed',
  deploymentUnavailable: "Deployment details aren't available for your role.",
  whatItDoes: 'What it does',
  noDescription: 'No description.',
  skills: 'Skills',
  noSkills: 'No skills advertised.',
  callThisAgent: 'Call this agent',
  tryIt: 'Try it',
  tryItTitle: (name: string) => `Chat with ${name} in the browser`,
  technical: 'Technical details',
  liveCard: 'Live agent card',
  liveCardUnavailable: 'Live card available when running.',
  viewRaw: 'View raw',
  learnMore: 'Learn more',
  version: 'Version',
  ownerLine: (owner: string) => `Owner: ${owner}`,
  updatedLine: (when: string) => `Updated ${when}`,
  yes: 'Yes',
  no: 'No',
  tech: {
    streaming: 'Streaming',
    push: 'Push notifications',
    history: 'State history',
    inputModes: 'Input modes',
    outputModes: 'Output modes',
    protocol: 'Protocol',
    transport: 'Transport',
    harness: 'Harness',
    documentation: 'Documentation',
  },
  copyExample: (ex: string) => `Copy example: ${ex}`,
  copyCommand: (cmd: string) => `Copy: ${cmd}`,

  stats: 'Last 24 hours',
  turns: 'Turns',
  cost: 'Est. cost',
  latencyP50: 'p50 latency',
  latencyP99: 'p99 latency',
  resources: 'Resources',
  noResources: 'Resource usage appears when the agent is running.',
  obsOff:
    'Observability is not configured: set OTEL_EXPORTER_OTLP_ENDPOINT, TEMPO_URL and LOKI_URL.',
  recentSessions: 'Recent sessions',
  noSessions: (n: number) => `No sessions for this agent in the latest ${n} sessions.`,
  nameCollision:
    "Matched by name; may include sessions of another owner's agent, or a deleted one, with the same name.",
  logs: 'Logs',
  noLogs: 'No log lines yet.',
  liveTail: 'Live tail',
  errorsDot: 'Errors in the latest log lines',
  cpu: 'CPU',
  memory: 'Memory',
  memoryLimit: 'Memory limit',
  level: 'Level',
  allLevels: 'All',
  searchLogs: 'Search logs',
  logLines: 'Log lines, newest first',

  versions: 'Versions',
  loadingVersions: 'Loading versions',
  noVersions: 'No versions yet (never deployed).',
  active: 'Active',
  rollBackTo: 'Roll back to this version',
  rollBackTitle: (v: string) => `Roll back to ${v}?`,
  rollBackBody: 'The agent redeploys this version. Callers may see errors while it restarts.',
  rollBackReason: 'Reason (optional)',
  rollBackQueued: (build: string) => `Roll back queued (build ${build}).`,
  stillRollingBack: (build: string) => `Still rolling back (build ${build}). Check the logs:`,
  keepWatching: 'Keep watching',
  rolledBack: 'Rolled back.',

  access: 'Access',
  publicToggle: 'Public: any signed-in user can find and call this agent',
  userGrants: 'Shared with users',
  noUserGrants: 'Not shared with anyone.',
  addUser: 'Add a user',
  searchUsers: 'Search users by name',
  typeMore: (n: number) => `Type at least ${n} characters.`,
  noUsers: 'No matching users you can share with.',
  // The agent-to-agent list means opposite things by edition (grants.ts). OSS: agent_acl rows are the agents THIS agent
  // may call; at ea233d20 the server records them but doesn't enforce them (a2a_dispatch passes no caller to
  // CpCallGuard; recommendation C-6). EE: 'agent' grants let the listed agents call this one, and only add access.
  agentAcl: 'Agents this one may call',
  noAcl: 'None listed.',
  aclNotEnforced:
    'This server records the list but doesn’t enforce it yet: agent calls aren’t checked against it.',
  addAgent: 'Allow an agent',
  agentAclEe: 'Agents allowed to call this one',
  noAclEe:
    'None listed. Agents listed here can call this one; public agents stay callable by anyone.',
  chooseAgent: 'Choose an agent…',
  allow: 'Allow',
  add: 'Add',
  removeNamed: (name: string) => `Remove ${name}`,
  addNamed: (name: string) => `Add ${name}`,
  removeSecret: (name: string) => `Remove secret ${name}`,
  remove: 'Remove',

  settings: 'Settings',
  displayName: 'Display name',
  description: 'Description',
  save: 'Save',
  saved: 'Saved',
  secrets: 'Secrets',
  noSecrets: 'No secrets set.',
  secretName: 'Name',
  secretValue: 'Value',
  addSecret: 'Add or replace',
  // Verified live (QA 2026-09-27): Restart redeploys and injects the new value; the running container doesn't see it before.
  secretsHowItWorks:
    'Available to the agent as environment variables. Restart or redeploy the agent to apply a change.',
  // Agent flags (catalog/routes.rs `AgentDetailResponse` at nasiko-cloud-rs 1a305a63): the legacy Settings tab's copy.
  features: 'Features',
  featuresHint: 'Agent-level feature flags. Changes take effect on next restart.',
  promptComments: 'Prompt comments',
  promptCommentsHint:
    'Lets the agent record, prune, and maintain workspace instructions with rationale annotations. Workspaces can opt out with',
  promptCommentsOptOut: '<!-- @prompt-comments disabled -->',
  tokenOptimization: 'Token optimization',
  tokenOptimizationIntro:
    'One switch over the whole stack: shrinks large tool results (JSON, logs and diffs) before they reach the model, trims the reply instruction, and compresses what the Orchestrator keeps between turns. Errors and structure are kept, and a counted note is left wherever something was removed. Turning it off stops every part of it.',
  tokenOptimizationHint: 'Off by default. Your own messages are never changed.',
  compactTools: 'Compact tool definitions',
  compactToolsHint:
    'Reduce the size of tool instructions sent to the model. Unsupported requests continue using their original format.',
  compactToolsCoverage:
    'Applies to OpenAI-style chat requests that are not streamed. Streamed requests, forced tool choices and JSON response formats keep their original format. Applies immediately.',
  compactToolsUnavailable:
    'Not enabled on this server. An operator can turn it on with TOKEN_COMPACT_TOOLS=true.',
  codingBehavior: 'Coding agent behavior',
  minimalCode: 'Minimal-code mode',
  minimalCodeHint:
    'Checks for existing code, the standard library, or an installed dependency before writing new code. Applies immediately, no restart needed.',
  selfReview: 'Self-review',
  selfReviewHint:
    'Adds a review turn that catches duplicated or unnecessary code — one extra model call per edit, so it is off until you ask for it. Needs Minimal-code mode. Restart the agent to apply.',
  dangerZone: 'Danger zone',
  deleteThisAgent: 'Delete this agent',
  deleteAgent: 'Delete agent',
  deleteTitle: (name: string) => `Delete ${name}?`,
  deleteBody: 'The container stops and callers get 404. Traces and past usage stay.',
  deleteConfirmLead: 'Type',
  deleteConfirmTail: 'to confirm',

  restart: 'Restart',
  stop: 'Stop',
  start: 'Start',
  stopTitle: (name: string) => `Stop ${name}?`,
  stopBody: 'The container stops; callers get errors until you start it again.',
  restarted: 'Restarted',
  started: 'Started',
  stoppedDone: 'Stopped',
  crashedAgain: 'Restarted, but the agent crashed again.',
  restarting: 'Restarting…',
  starting: 'Starting…',
  stillStarting: 'Still starting. Check the logs.',
  couldntRefresh: "Couldn't refresh.",
  copyCli: 'Copy CLI command',
  copyUuid: 'Copy ID',
  copyLink: 'Copy link',
  copied: 'Copied',
  copyFailed: 'Copy failed — select and copy manually.',
  cliNote: "Runs against your CLI's active cluster. Run `nasiko connect <origin>` first if needed.",
  openAgent: 'Open',
  viewLogs: 'View logs',
  details: 'Details',
  cancel: 'Cancel',
  retry: 'Retry',
  serverDown: 'OpenRuntime server unreachable.',
  somethingWrong: 'Something went wrong.',
} as const

/** What the user reads for a failed call (plan §6.4): our copy first, server text behind Details. */
export interface ErrorCopy {
  message: string
  detail: string | null
}

export type ErrorContext = 'manage' | 'lifecycle' | 'rollback' | 'secret' | 'read'

export function errorCopy(err: unknown, context: ErrorContext = 'read'): ErrorCopy {
  const api = err instanceof ApiError ? err : null
  const detail = serverDetail(api)
  if (!api)
    return {
      message: copy.somethingWrong,
      detail: err instanceof Error ? err.message.slice(0, SERVER_TEXT_MAX) : null,
    }
  const s = api.status
  if (s === 401) return { message: 'Your session expired. Sign in again.', detail }
  if (s === 403) {
    return {
      message:
        context === 'lifecycle' || context === 'rollback'
          ? "Your role can't deploy. Ask an admin."
          : 'Only the owner or a superuser can change this agent.',
      detail,
    }
  }
  if (s === 404) return { message: `${copy.notFound} ${copy.notFoundFix}`, detail }
  if (s === 400 && context === 'rollback')
    return { message: "This version can't be rolled back to.", detail }
  if (s === 409 && context === 'rollback')
    return { message: 'Another build may be in progress.', detail }
  if (s === 422 && context === 'secret')
    return { message: 'Names use letters, digits and underscores.', detail }
  if (s === 500 && context === 'lifecycle')
    return {
      message:
        "The runtime couldn't find this agent's container. Redeploy it with `nasiko deploy`.",
      detail,
    }
  if (s === 503) return { message: copy.obsOff, detail }
  if (api.isServerUnreachable) return { message: copy.serverDown, detail }
  return { message: copy.somethingWrong, detail }
}

/** Plain-text server bodies only, cut short (HTML error pages and JSON envelopes are dropped). */
function serverDetail(api: ApiError | null): string | null {
  if (!api || typeof api.body !== 'string') return null
  const text = api.body.trim()
  if (!text || text.startsWith('<')) return null
  return text.length > SERVER_TEXT_MAX ? `${text.slice(0, SERVER_TEXT_MAX)}…` : text
}

/** OSS docs the sections link to (public repo paths). */
export const DOCS = {
  lifecycle:
    'https://github.com/Nasiko-Labs/nasiko/blob/main/docs/AGENT_LIFECYCLE.md#phase-3-deploy',
  operate:
    'https://github.com/Nasiko-Labs/nasiko/blob/main/docs/AGENT_LIFECYCLE.md#lifecycle-management',
  registry: 'https://github.com/Nasiko-Labs/nasiko/blob/main/docs/A2A_REGISTRY_DESIGN.md',
  access:
    'https://github.com/Nasiko-Labs/nasiko/blob/main/docs/ROUTING_ENGINE.md#access-control-pre-filter',
  secrets: 'https://github.com/Nasiko-Labs/nasiko/blob/main/docs/AGENT_LIFECYCLE.md#manage-secrets',
  versions:
    'https://github.com/Nasiko-Labs/nasiko/blob/main/docs/AGENT_LIFECYCLE.md#iterating-on-a-deployed-agent',
} as const
