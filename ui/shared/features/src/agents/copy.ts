/**
 * Every user-facing string on the agent pages (plan §6.4, DX). Retired wording ("Shared with
 * you", "last deployed", a "Registered" badge) must not come back; logic.test.ts checks. Status
 * labels and hints live in the status matrix (status.ts), the single status source.
 */
import { ApiError } from '@nasiko/data-access/client'
import { isObservabilityUnconfigured } from '@nasiko/features/observability/unconfigured'
import { SERVER_TEXT_MAX } from './tuning'

export const copy = {
  catalogTitle: 'Agents',
  catalogSub: 'Discover, inspect and manage every agent in your workspace',
  mineTitle: 'Your agents',
  // The module's sub-nav (components/bits AgentsNav): Deploy and Builds moved here from the sidebar.
  sections: { label: 'Agents', all: 'All agents', mine: 'Your agents', builds: 'Builds' },
  searchLabel: 'Search agents',
  searchPlaceholder: 'Search by name, description, tag or skill',
  yours: 'Yours',
  availableToYou: 'Available to you',
  publicChip: 'Public',
  harnessChip: 'Coding harness',
  harnessesToggle: (n: number) => `Coding harnesses (${n})`,
  // With the `agentPlacement` slot (MT): where each agent runs (FR-4, TM-5).
  local: 'Local',
  cloud: 'Cloud',
  localToggle: (n: number) => `Local agents (${n})`,
  ownerLabel: 'Owner',
  ownerFilter: 'Filter by owner',
  anyone: 'Anyone',
  tagLabel: 'Tag',
  tagFilter: 'Filter by tag',
  allTags: 'All',
  /** The catalog's view switch: the cards, or one table row per agent. */
  viewLabel: 'View',
  viewGrid: 'Grid',
  viewList: 'List',
  colDescription: 'Description',
  colSkills: 'Skills',
  harnessUsage: 'See usage on Coding harnesses',
  excludesHarnesses: (n: number) => `Excludes ${n} coding harness${n === 1 ? '' : 'es'}`,
  noResults: 'No agents match this search',
  noResultsHint: 'Try another name, tag or skill, or clear the search and filters.',
  clearSearch: 'Clear search',
  /** The health filter: the home's rating badges link here (overview design 15A; plans/feat-agent-centric.md PR2a). */
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
  tabHarnesses: 'Coding harnesses',
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
  tabDetails: 'Details',

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
  overviewSummary: 'Summary',
  allDetails: 'All details',
  noDescription: 'No description.',
  skills: 'Skills',
  noSkills: 'No skills advertised.',
  showMore: 'Show more',
  showLess: 'Show less',
  showAllSkills: (n: number) => `Show all ${n} skills`,
  callThisAgent: 'Call this agent',
  tryIt: 'Try it',
  tryItTitle: (name: string) => `Chat with ${name} in the browser`,
  technical: 'Technical details',
  liveCard: 'Live agent card',
  liveCardUnavailable: 'Live card available when running.',
  viewRaw: 'View raw',
  copyLiveCard: 'Copy the live agent card',
  copyRaw: 'Copy the raw responses',
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

  summary: 'Usage and resources',
  stats: 'Last 24 hours',
  turns: 'Turns',
  cost: 'Est. cost',
  latencyP50: 'p50 latency',
  latencyP99: 'p99 latency',
  resources: 'Resources',
  noResources: 'Resource usage appears when the agent is running.',
  obsOff:
    'Observability is not configured: set OTEL_EXPORTER_OTLP_ENDPOINT, TEMPO_URL and LOKI_URL.',
  logs: 'Logs',
  noLogs: 'No log lines yet.',
  liveTail: 'Live tail',
  errorsDot: 'Errors in the latest log lines',
  cpu: 'CPU',
  memory: 'Memory',
  memoryOfLimit: (pct: number) => `Memory: ${pct}% of the limit`,
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
  rollBackReasonPlaceholder: 'e.g. The new version fails on long inputs',
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
  displayNamePlaceholder: 'e.g. Support bot',
  description: 'Description',
  descriptionPlaceholder: 'What this agent does, in a sentence or two',
  save: 'Save',
  saved: 'Saved',
  secrets: 'Secrets',
  noSecrets: 'No secrets set.',
  secretName: 'Name',
  secretNamePlaceholder: 'API_KEY',
  secretValue: 'Value',
  secretValuePlaceholder: 'Paste the value',
  addSecret: 'Add or replace',
  // Verified live (QA 2026-09-27): Restart redeploys and injects the new value; the running container doesn't see it before.
  // NAS-1489: say the agent uses them at runtime. Agent secrets list names only (agent_secrets.rs `list_secrets`).
  secretsHowItWorks:
    'The agent reads these as environment variables at runtime. Values can’t be read back; Add or replace overwrites one. Restart or redeploy the agent to apply a change.',
  // NAS-1458. agent_secrets.rs `resolve_secret_env`: the owner's user secrets first, the agent's own on top.
  secretsPrecedence:
    'The agent also gets every secret its owner keeps in Settings → Secrets. A secret set here overrides one of the same name there, for this agent only.',
  inheritedSecrets: 'From the owner’s secrets',
  inheritedSecretsLink: 'Manage in Settings → Secrets',
  noInheritedSecrets: 'None. Secrets the owner adds in Settings → Secrets will appear here.',
  inheritedBadge: 'Inherited',
  overriddenBadge: 'Overridden here',
  overridesBadge: 'Overrides owner’s secret',
  // Agent flags (catalog/routes.rs `AgentDetailResponse` at nasiko-cloud-rs 1a305a63): the legacy Settings tab's copy.
  features: 'Features',
  featuresHint: 'Agent-level feature flags. Changes take effect on next restart.',
  promptComments: 'Prompt comments',
  promptCommentsHint:
    'Lets the agent record, prune, and maintain workspace instructions with rationale annotations. Workspaces can opt out with',
  promptCommentsOptOut: '<!-- @prompt-comments disabled -->',
  codingBehavior: 'Coding agent behavior',
  minimalCode: 'Minimal-code mode',
  minimalCodeHint:
    'Checks for existing code, the standard library, or an installed dependency before writing new code. Applies immediately, no restart needed.',
  selfReview: 'Self-review',
  selfReviewHint:
    'Adds a review turn that catches duplicated or unnecessary code — one extra model call per edit, so it is off until you ask for it. Needs Minimal-code mode. Restart the agent to apply.',
  dangerZone: 'Danger zone',
  // plans/feat-context-optimization.md eng E1 (D3), E2 (D4): the switch does three things; the label says so. One switch
  // (TokenOptimization.tsx); main's second copy in the feature flags was folded into it at the integration (2026-10-05).
  // Named with the program it turns on (plans/feat-optimization-page.md B12).
  tokenOptimization: 'Token optimization',
  tokenOptimizationIntro:
    'One switch over the whole stack: shrinks large tool results (JSON, logs and diffs) before they reach the model, trims the reply instruction, and compresses what the Orchestrator keeps between turns. Errors and structure are kept, and a counted note is left wherever something was removed. Turning it off stops every part of it.',
  /** The switch's own label, so the section title isn't printed twice (review: design). */
  tokenOptimizationSwitch: 'On for this agent',
  tokenOptimizationHint:
    "Compresses this agent's LLM requests and asks it for concise answers (through the LLM router). History compression also needs it on for all of an owner's agents.",
  tokenOptimizationHarness: 'For a coding harness this only affects history compression.',
  tokenOptimizationFailed: (reason: string) => `Couldn't change Token optimization: ${reason}`,
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
  stopSharedWarning:
    'Others use this agent: it is shared with people besides you. They will see it was stopped by its owner and cannot chat with it until you start it again.',
  stoppedByOwner: 'This agent was stopped by its owner. You can use it again once they start it.',
  // NAS-1487: what Parked means and what to do, under the detail header; the switch is on the Settings tab.
  parkedTitle: 'Parked while idle',
  parkedBody:
    'Nobody called this agent for a while, so its container was released to save resources. Nothing is lost: the next message starts it again, and that first reply takes a little longer.',
  parkedStart: 'Select Start to bring it up now.',
  parkedKeepRunning: 'Select Start to bring it up now, or keep it running with Always on in',
  availability: 'Availability',
  alwaysOn: 'Always on',
  alwaysOnHint:
    'Off: when nobody has called it for a while, the agent is parked and the next message starts it again. On: it keeps running, and using resources, while idle.',
  alwaysOnUnavailable: 'This workspace parks every idle agent, so none can be kept always on.',
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
  serverDown: 'Open Runtime server unreachable.',
  somethingWrong: 'Something went wrong.',
  /** The server's own reason, when it gave one we have no better words for. */
  serverSaid: (t: string) => `Open Runtime said: ${/[.!?]$/.test(t) ? t : `${t}.`}`,
} as const

/**
 * What the user reads for a failed call (plan §6.4): our copy first, server text behind Details. When we have no
 * words of our own for it, the server's reason is the message itself (NAS-1624), never "Something went wrong".
 */
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
  // A missing container answers a bare "internal error" (containers.rs); any other reason is the real one.
  if (s === 500 && context === 'lifecycle' && (!detail || GENERIC_500.test(detail)))
    return {
      message:
        "The runtime couldn't find this agent's container. Redeploy it with `nasiko deploy`.",
      detail,
    }
  if (isObservabilityUnconfigured(api)) return { message: copy.obsOff, detail }
  if (api.isServerUnreachable && !detail) return { message: copy.serverDown, detail }
  if (detail && !GENERIC_500.test(detail)) return { message: copy.serverSaid(detail), detail: null }
  return { message: copy.somethingWrong, detail }
}

/** The server's catch-all 500 text: it names no cause, so it never becomes the message. */
const GENERIC_500 = /^internal (server )?error\.?$/i

/** The server's reason (plain text, `{message}`, `{error}`), cut short; HTML error pages are dropped. */
function serverDetail(api: ApiError | null): string | null {
  const text = api?.serverMessage
  if (!text) return null
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
