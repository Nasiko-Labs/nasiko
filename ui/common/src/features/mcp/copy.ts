/**
 * Every user-facing string on the MCP pages (plans/feat-mcp.md). Worded as nasiko-cloud-rs `2d6178e4` mcp-page.js,
 * mcp-detail-page.js and the agent page's MCP section say it, except where the legacy wording didn't fit the new layout.
 */
import { ApiError, isTimeoutError } from '@/lib/api/client'
import type { ServerStatus } from './logic'
import type { Tab, View } from './search'
import type { AuthType, Stance } from './types'

export const AUTH_LABEL: Record<string, string> = {
  none: 'No auth',
  bearer: 'API key',
  basic: 'Basic',
  oauth2: 'OAuth 2.1',
  url_param: 'URL param',
}

/** The register form's longer names (legacy select). */
export const AUTH_OPTION: Record<AuthType, string> = {
  none: 'No auth',
  bearer: 'API key (bearer)',
  basic: 'Basic auth',
  oauth2: 'OAuth 2.1',
  url_param: 'URL parameter',
}

export const STATUS_LABEL: Record<ServerStatus, string> = {
  active: 'Active',
  inactive: 'Inactive',
  building: 'Building',
  failed: 'Build failed',
}

export const STANCE: Record<Stance, { label: string; title: string }> = {
  allow: { label: 'Allow', title: 'Runs without asking' },
  ask: { label: 'Ask', title: 'Pauses and asks you to approve each call' },
  block: { label: 'Block', title: 'Never runs' },
}

export const copy = {
  title: 'MCP servers',
  sub: 'Connect MCP servers and choose which agents can use their tools',
  upload: 'Upload MCP server',
  register: 'Register server',
  searchLabel: 'Search MCP servers',
  searchPlaceholder: 'Search servers and toolkits',
  views: { yours: 'Yours', shared: 'Shared with you', toolkits: 'Toolkits' } satisfies Record<
    View,
    string
  >,
  tabs: { all: 'All', available: 'Available to connect', connected: 'Connected' } satisfies Record<
    Tab,
    string
  >,
  tabsLabel: 'Connection',
  count: (n: number) => `${n} ${n === 1 ? 'server' : 'servers'}`,
  tools: (n: number) => `${n} ${n === 1 ? 'tool' : 'tools'}`,
  yours: 'Yours',
  sharedBy: (owner: string | null) => `Shared by ${owner || 'someone'}`,
  sharedWithYou: 'Shared with you',
  toolkit: 'Toolkit',
  publicChip: 'Public',
  noDescription: 'No description provided.',
  building: 'Building and deploying…',
  buildingHint: 'This may take a few minutes. Status updates automatically.',
  buildFailed: 'Build failed',
  buildFailedHint: 'Open the server to see its build logs.',
  loadFailed: "Couldn't load the catalog",
  loadFailedFix: 'Something went wrong loading connectable servers and toolkits.',
  toolkitsFailed: "Couldn't load toolkits. Custom servers are shown.",
  retry: 'Retry',
  clearSearch: 'Clear search',
  loadingCatalog: 'Loading MCP servers',
  loadingServer: 'Loading MCP server',
  loadingLogs: 'Loading build logs',
  noResults: (q: string) => `No servers match “${q}”`,
  noResultsHint: 'Search by server name or description, or clear the search.',
  empty: {
    all: {
      title: 'No connectable services yet',
      desc: 'Register an external MCP server or upload your own to give agents new tools.',
    },
    yours: {
      title: 'No servers of your own yet',
      desc: 'Register an external MCP server or upload your own. Anything you add shows up here with the tools it exposes.',
    },
    shared: {
      title: 'Nothing shared with you yet',
      desc: 'When a teammate shares one of their MCP servers, it lands here and its tools become available to your agents.',
    },
    toolkits: {
      title: 'No toolkits yet',
      desc: 'Platform-registered app toolkits (Gmail, Notion, …) show up here once an admin adds them.',
    },
  },
  nothingConnected: 'Nothing connected yet',
  nothingConnectedDesc:
    'Connect a server or toolkit and its tools become available to your agents.',
  browseAvailable: 'Browse available',
  allConnected: 'Everything is connected',
  allConnectedDesc: 'Every server and toolkit in this view is already connected.',

  // Connect / disconnect
  connect: 'Connect',
  connectNamed: (name: string) => `Connect ${name}`,
  connected: 'Connected',
  disconnect: 'Disconnect',
  disconnectNamed: (name: string) => `Disconnect ${name}`,
  disconnectBody: 'Agents lose access to its tools until you reconnect.',
  cancel: 'Cancel',
  keyLabel: 'API key / token',
  basicPlaceholder: 'username:password',
  connectedToast: (name: string) => `${name} connected`,
  disconnectedToast: (name: string) => `${name} disconnected`,
  oauthOpened: 'Finish signing in in the window that opened. This page updates when it closes.',
  popupBlocked: 'Your browser blocked the sign-in window.',
  openSignIn: 'Open the sign-in page',
  unsafeUrl: "The server sent a sign-in address that isn't https, so it wasn't opened.",
  connectFailed: (reason: string) => `Connect failed: ${reason}`,
  disconnectFailed: (reason: string) => `Disconnect failed: ${reason}`,

  // Register
  registerTitle: 'Register server',
  registerDesc: 'Add an external MCP server by URL. You can set its credential after registering.',
  fields: {
    name: { label: 'Name', placeholder: 'github' },
    display_name: { label: 'Display name', placeholder: 'GitHub' },
    url: { label: 'Server URL', placeholder: 'https://mcp.example.com/mcp' },
    auth_type: { label: 'Auth type' },
    credential_header_name: {
      label: 'Credential header',
      hint: 'Optional. Defaults to Authorization.',
      placeholder: 'X-Api-Key',
    },
    basic_username: { label: 'Username' },
    basic_password: { label: 'Password' },
    oauth_client_id: {
      label: 'OAuth client ID',
      hint: 'Leave blank if the server supports automatic client registration (DCR).',
    },
    oauth_client_secret: { label: 'OAuth client secret' },
    url_param_name: { label: 'URL parameter name', placeholder: 'api_key' },
    description: { label: 'Description', placeholder: "What this server's tools do" },
  },
  required: 'Enter a value.',
  badUrl: 'Enter an http(s) URL.',
  probe: 'Probe',
  probing: 'Probing…',
  probeResult: (auth: string) => `Detected auth: ${auth}`,
  probeFailed: (reason: string) => `Probe failed: ${reason}`,
  registering: 'Registering…',
  registerFailed: (reason: string) => `Couldn't register: ${reason}`,
  registered: (name: string) => `${name} registered`,

  // Upload
  uploadTitle: 'Upload MCP server',
  zipTitle: 'Upload a zip',
  zipDesc: 'Upload a .zip archive containing your MCP server source code',
  githubTitle: 'Import from GitHub',
  githubDesc: 'Clone a GitHub repository containing your MCP server',
  file: 'Source archive (.zip)',
  nameHint: 'Filled from the file name. You can change it.',
  uploadName: { label: 'Name', placeholder: 'my-mcp-server' },
  versionTag: { label: 'Version tag', placeholder: 'v1' },
  githubUrl: { label: 'GitHub repository URL', placeholder: 'https://github.com/org/repo' },
  back: 'Back',
  uploadSubmit: 'Upload and build',
  uploading: 'Uploading…',
  chooseZip: 'Choose a .zip file.',
  uploadFailed: (reason: string) => `Upload failed: ${reason}`,
  uploadQueued: (name: string) => `${name} is building`,
  open: 'Open',
  noDeployRights: "You don't have permission to build MCP servers. Ask an admin for deploy rights.",

  // Detail
  notFound: 'MCP server not found',
  notFoundFix: 'It may have been deleted, or you no longer have access to it.',
  backToCatalog: 'MCP servers',
  ownerLine: (who: string) => `Owner: ${who}`,
  you: 'you',
  copyId: 'Copy server ID',
  copyLink: 'Copy link',
  moreActions: (name: string) => `More actions for ${name}`,
  tabsNames: {
    overview: 'Overview',
    agents: 'Agents',
    access: 'Access',
    logs: 'Logs',
    settings: 'Settings',
  },
  facts: 'Details',
  factLabels: {
    url: 'URL',
    transport: 'Transport',
    auth: 'Auth type',
    version: 'Version',
    source: 'Source',
    owner: 'Owner',
    tools: 'Tools',
    created: 'Created',
  },
  uploaded: 'Uploaded build',
  registeredSource: 'Registered',
  toolsTitle: (n: number) => `Tools (${n})`,
  noTools: 'No tools discovered yet',
  filterTools: 'Filter tools',
  noToolMatch: 'No tools match.',
  buildError: (msg: string) => `Build error: ${msg}`,

  // Your connection
  connection: 'Your connection',
  noAuthNeeded: 'This server needs no credential.',
  credentialSet: 'Credential set',
  noCredential: 'No credential set',
  credentialLabel: 'Credential',
  save: 'Save',
  replace: 'Replace',
  remove: 'Remove',
  verifyFailed: (err: string | null | undefined) =>
    `Stored, but verification failed: ${err || 'unknown error'}`,
  credentialSaved: 'Credential saved and verified',
  saveFailed: (reason: string) => `Couldn't save: ${reason}`,
  removeFailed: (reason: string) => `Couldn't remove: ${reason}`,
  oauthTitle: 'OAuth 2.1',
  authorized: 'Authorized',
  expires: (when: string) => `expires ${when}`,
  notAuthorized: 'Not authorized',
  authorize: 'Authorize',
  revoke: 'Revoke',
  authorizeFailed: (reason: string) => `Authorization failed: ${reason}`,
  revokeFailed: (reason: string) => `Revoke failed: ${reason}`,
  statusFailed: "Couldn't check your connection.",

  // Danger zone
  danger: 'Danger zone',
  dangerBody: 'Deleting this server removes it and revokes all agent access.',
  delete: 'Delete server',
  deleteTitle: (name: string) => `Delete ${name}?`,
  deleteBody: 'This removes the server and revokes all agent access. This cannot be undone.',
  deleting: 'Deleting…',
  deleted: (name: string) => `${name} deleted`,
  deleteFailed: (reason: string) => `Couldn't delete: ${reason}`,

  // Agents tab + agent MCP tab
  agentAccess: 'Agent access',
  agentAccessSub: "Choose an agent to manage its access to this server's tools.",
  agentLabel: 'Agent',
  chooseAgent: 'Choose an agent',
  searchAgents: 'Search agents…',
  noAgents: 'No agents match.',
  consumersTitle: 'Agents using this server',
  noConsumers: 'No agent has this server configured yet.',
  toolsOf: (used: number, total: number) => `${used} of ${total} tools`,
  manage: 'Manage',
  notConnectedNote:
    "You haven't connected this server, so agents can't call its tools as you yet. Connect it first.",
  agentMcpTitle: 'MCP servers',
  agentMcpSub:
    'MCP servers this agent may use. Set each tool to allow, ask or block. Ask pauses the agent mid-task and asks you to approve that call before it runs.',
  noAgentServers: 'No MCP servers available',
  noAgentServersFix: 'Connect servers on the MCP servers page to make their tools available here.',
  openCatalog: 'Open MCP servers',
  enable: (name: string) => `Enable ${name}`,
  disable: (name: string) => `Disable ${name}`,
  expand: (name: string) => `Show tools for ${name}`,
  collapse: (name: string) => `Hide tools for ${name}`,
  summaryDisabled: 'Disabled',
  enabled: 'Enabled',
  summaryNoTools: 'No tools synced yet',
  summary: (allowed: number, total: number, asks: number) =>
    `${allowed} of ${total} tools allowed${asks ? ` · ${asks} ask first` : ''}`,
  notReadyBuilding: 'Still building. Agents can use it once the build finishes.',
  notReadyFailed: 'The build failed, so agents can’t use it.',
  viewServer: 'View server',
  viewLogs: 'View build logs',
  disabledNote: 'This server is disabled for this agent. Tool rules apply once it is re-enabled.',
  noToolsYet: 'This server exposes no tools yet.',
  stanceFor: (tool: string) => `Tool access for ${tool}`,
  toolsFailed: "Couldn't load this server's tools",
  agentFailed: "Couldn't load this agent's MCP servers",
  accessFailed: (reason: string) => `Couldn't update access: ${reason}`,
  ruleFailed: (reason: string) => `Couldn't save the tool rule: ${reason}`,

  // Access tab
  publicTitle: 'Visibility',
  publicToggle: 'Public: everyone on this OpenRuntime can use this server',
  grantsTitle: 'Who has access',
  grantsSub: 'Who this server is shared with.',
  searchGrants: 'Search people',
  grantCols: { user: 'User', email: 'Email', role: 'Role', grant: 'Grant' },
  grantKind: { owner: 'Owner', direct: 'Direct', inherited: 'Inherited', public: 'Public' },
  noGrants: 'No one else has access yet. Use Grant access to share this server.',
  noGrantMatch: 'No one matches.',
  revokeNamed: (name: string) => `Revoke access for ${name}`,
  revokeTitle: 'Revoke access',
  revokeBody: (name: string) => `Revoke access for ${name}? They lose access to this MCP server.`,
  revoked: 'Access revoked',
  grantTitle: 'Grant access',
  grantUser: 'User',
  searchUsers: 'Search users',
  typeMore: (n: number) => `Type at least ${n} characters.`,
  noUsers: 'No users match.',
  grant: 'Grant',
  grantNamed: (name: string) => `Grant access to ${name}`,
  granted: (name: string) => `Access granted to ${name}`,
  grantFailed: (reason: string) => `Couldn't grant access: ${reason}`,
  grantsFailed: "Couldn't load grants for this server",

  // Logs tab
  logsTitle: 'Build logs',
  buildStatusLine: (s: string) => `Build status: ${s}`,
  imageTag: (t: string) => `Image ${t}`,
  noLogs: '(no logs)',
  refresh: 'Refresh',
  copyLogs: 'Copy logs',
  logsFailed: "Couldn't load the build logs",

  // Settings tab
  settingsTitle: 'Server settings',
  active: 'Active',
  activeHint: 'An inactive server offers no tools to any agent.',
  saveChanges: 'Save changes',
  saving: 'Saving…',
  saved: 'Server updated',
  nothingChanged: 'Nothing to save.',
} as const

/** The server's own words when it sent any (the gateway's `client_message` is safe to show), else a fallback. */
export function reason(err: unknown): string {
  if (isTimeoutError(err)) return 'OpenRuntime took too long to answer.'
  if (err instanceof ApiError) {
    if (err.status === 403 && !err.serverMessage) return "you don't have permission"
    if (err.serverMessage) return err.serverMessage
    if (err.isServerUnreachable) return "OpenRuntime isn't answering."
    return `error ${err.status}`
  }
  return err instanceof Error ? err.message : 'unknown error'
}
