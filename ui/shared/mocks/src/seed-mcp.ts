/**
 * The MCP seed (plans/feat-mcp.md §8): one description read by the mock (`mcp.ts`) and by `seed:live`
 * (`scripts/seed-trace-usage.ts`), so a recorded fixture's ids are the ids the mock answers with. Like `seed.ts` it
 * stays import-free, erasable-only TypeScript that Node runs directly.
 *
 * `live: false` rows exist only in the mock: the live seed has one user (the superuser), no Composio tool sync and no
 * credential encryption key, so servers owned by other people, a build in progress and a stored credential can't be
 * written there. Connector ids are `5eed000e-*`, build ids `5eed000f-*`.
 */

interface McpSeedTool {
  name: string
  description: string
}

export interface McpSeedConnector {
  n: number
  name: string
  display_name: string
  description: string
  provider: 'composio' | 'mcp_server'
  url: string | null
  auth_type: 'none' | 'bearer' | 'basic' | 'oauth2' | 'url_param' | null
  credential_header_name: string | null
  source_kind: 'external_url' | 'uploaded_build'
  /** `admin` is the superuser; the others are harness seed users (mock only). */
  owner: 'admin' | 'other0' | 'other1' | null
  public: boolean
  /** A user grant to the admin (on a server someone else owns) or to `other0` (on the admin's). */
  sharedWith: 'admin' | 'other0' | null
  daysAgo: number
  tools: McpSeedTool[]
  /** Uploads: the latest build. A failed one follows a successful `prior` (a failed first upload is deleted). */
  build: {
    version: string
    status: 'running' | 'failed' | 'building'
    prior: string | null
    error: string | null
  } | null
  live: boolean
}

const pad = (n: number) => String(n).padStart(12, '0')
export const mcpConnectorId = (n: number) => `5eed000e-0000-4000-8000-${pad(n)}`
export const mcpBuildId = (n: number) => `5eed000f-0000-4000-8000-${pad(n)}`

const t = (...pairs: [string, string][]): McpSeedTool[] =>
  pairs.map(([name, description]) => ({ name, description }))

const base = {
  display_name: '',
  url: null,
  auth_type: 'none',
  credential_header_name: null,
  source_kind: 'external_url',
  owner: 'admin',
  public: false,
  sharedWith: null,
  build: null,
  live: true,
} as const

const toolkit = (
  n: number,
  name: string,
  display: string,
  description: string,
  tools: McpSeedTool[],
) =>
  ({
    ...base,
    n,
    name,
    display_name: display,
    description,
    provider: 'composio',
    auth_type: null,
    owner: null,
    daysAgo: 30,
    tools,
  }) satisfies McpSeedConnector

export const MCP_CONNECTORS: McpSeedConnector[] = [
  toolkit(
    1,
    'github',
    'GitHub',
    'Repositories, issues and pull requests.',
    t(
      ['create_issue', 'Open an issue in a repository.'],
      ['list_pull_requests', 'List open pull requests.'],
      ['merge_pull_request', 'Merge a pull request.'],
      ['delete_repo', 'Delete a repository.'],
    ),
  ),
  toolkit(
    2,
    'gmail',
    'Gmail',
    'Read, search and send email.',
    t(['send_email', 'Send an email.'], ['search_threads', 'Search mail threads.']),
  ),
  toolkit(
    3,
    'slack',
    'Slack',
    'Post and read channel messages.',
    t(
      ['post_message', 'Post to a channel.'],
      ['list_channels', 'List channels.'],
      ['read_thread', 'Read a thread.'],
    ),
  ),
  toolkit(
    4,
    'notion',
    'Notion',
    'Pages and databases in your workspace.',
    t(['search_pages', 'Search pages.'], ['create_page', 'Create a page.']),
  ),
  toolkit(
    5,
    'linear',
    'Linear',
    'Issues, projects and cycles.',
    t(['create_issue', 'Create an issue.'], ['update_issue', 'Update an issue.']),
  ),
  {
    ...base,
    n: 6,
    name: 'docs-search',
    display_name: 'Docs search',
    description: 'Search and read the internal documentation site.',
    provider: 'mcp_server',
    url: 'https://docs-mcp.example.com/mcp',
    sharedWith: 'other0',
    daysAgo: 14,
    tools: t(
      ['search_docs', 'Full-text search across every space.'],
      ['get_page', 'Read one page as Markdown.'],
      ['list_spaces', 'List documentation spaces.'],
      ['summarize_page', 'Summarize a page in a few sentences.'],
    ),
  },
  {
    ...base,
    n: 7,
    name: 'jira-cloud',
    display_name: 'Jira Cloud',
    description: 'Issues and sprints in Jira, signed in with your Atlassian account.',
    provider: 'mcp_server',
    url: 'https://mcp.atlassian.example.com/v1/mcp',
    auth_type: 'oauth2',
    daysAgo: 13,
    tools: t(
      ['search_issues', 'Search issues with JQL.'],
      ['create_issue', 'Create an issue.'],
      ['transition_issue', 'Move an issue to another status.'],
      ['add_comment', 'Comment on an issue.'],
      ['list_sprints', 'List sprints on a board.'],
      ['get_issue', 'Read one issue.'],
    ),
  },
  {
    ...base,
    n: 8,
    name: 'weather-api',
    display_name: 'Weather',
    description: 'Current conditions and forecasts.',
    provider: 'mcp_server',
    url: 'https://weather-mcp.example.com/mcp',
    auth_type: 'bearer',
    credential_header_name: 'X-Api-Key',
    daysAgo: 12,
    tools: t(
      ['current', 'Current conditions for a place.'],
      ['forecast', 'A 7-day forecast.'],
      ['alerts', 'Active weather alerts.'],
    ),
  },
  {
    ...base,
    n: 9,
    name: 'pdf-tools',
    display_name: 'PDF tools',
    description: 'Extract text, tables and images from PDFs.',
    provider: 'mcp_server',
    url: 'http://mcp-pdf-tools:8080/mcp',
    source_kind: 'uploaded_build',
    daysAgo: 11,
    tools: t(
      ['extract_text', 'Plain text of every page.'],
      ['extract_tables', 'Tables as CSV.'],
      ['page_count', 'How many pages a PDF has.'],
      ['split', 'Split a PDF into page ranges.'],
      ['merge', 'Merge PDFs into one.'],
    ),
    build: { version: 'v2', status: 'running', prior: null, error: null },
  },
  {
    ...base,
    n: 10,
    name: 'sql-runner',
    display_name: 'SQL runner',
    description: 'Read-only queries against the analytics warehouse.',
    provider: 'mcp_server',
    url: 'http://mcp-sql-runner:8080/mcp',
    source_kind: 'uploaded_build',
    daysAgo: 10,
    tools: t(
      ['run_query', 'Run a read-only SQL query.'],
      ['list_tables', 'List warehouse tables.'],
    ),
    build: {
      version: 'v2',
      status: 'failed',
      prior: 'v1',
      error: 'docker build failed: step 4/7 RUN pip install -r requirements.txt exited with 1',
    },
  },
  {
    ...base,
    n: 11,
    name: 'crm-sync',
    display_name: 'CRM sync',
    description: 'Accounts and contacts from the CRM.',
    provider: 'mcp_server',
    source_kind: 'uploaded_build',
    daysAgo: 0,
    tools: t(
      ['find_account', 'Look up an account.'],
      ['list_contacts', 'Contacts of an account.'],
      ['log_activity', 'Log a call or meeting.'],
    ),
    build: { version: 'v1', status: 'building', prior: null, error: null },
    live: false,
  },
  {
    ...base,
    n: 12,
    name: 'finance-ledger',
    display_name: 'Finance ledger',
    description: 'Invoices and ledger entries (finance team).',
    provider: 'mcp_server',
    url: 'https://ledger-mcp.example.com/mcp',
    auth_type: 'bearer',
    owner: 'other0',
    sharedWith: 'admin',
    daysAgo: 8,
    tools: t(
      ['list_invoices', 'Invoices in a period.'],
      ['get_invoice', 'One invoice.'],
      ['ledger_balance', 'Balance of a ledger account.'],
      ['create_journal_entry', 'Post a journal entry.'],
    ),
    live: false,
  },
  {
    ...base,
    n: 13,
    name: 'wiki-reader',
    display_name: 'Wiki reader',
    description: 'Read-only access to the team wiki.',
    provider: 'mcp_server',
    url: 'https://wiki-mcp.example.com/mcp',
    owner: 'other1',
    public: true,
    daysAgo: 7,
    tools: t(['search', 'Search the wiki.'], ['read', 'Read a page.']),
    live: false,
  },
]

/** The admin's connections. A stored credential needs the server's encryption key, so that one is mock only. */
export const MCP_CONNECTIONS: { name: string; credential: string | null; live: boolean }[] = [
  { name: 'github', credential: null, live: true },
  { name: 'docs-search', credential: null, live: true },
  { name: 'weather-api', credential: 'wx-live-key', live: false },
]

/** `mcp_agent_connector_access` rows, by index into the live (not deleted) seed agents. */
export const MCP_ACCESS: {
  agent: number
  name: string
  enabled: boolean
  rules: Record<string, 'allow' | 'ask' | 'block'>
}[] = [
  {
    agent: 0,
    name: 'docs-search',
    enabled: true,
    rules: { get_page: 'allow', summarize_page: 'ask' },
  },
  { agent: 0, name: 'weather-api', enabled: true, rules: {} },
  { agent: 0, name: 'github', enabled: true, rules: { delete_repo: 'block' } },
  { agent: 1, name: 'docs-search', enabled: false, rules: {} },
]
