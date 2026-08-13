// MCP gateway fixtures — shapes match the /api/mcp handlers
// (oss/server/src/mcp/handlers/*, views in oss/mcp-gateway/src/connectors.rs
// `connector_dto` / `list_connectors_view`, toolkits in
// oss/mcp-gateway/src/catalog.rs `list_toolkits_view`). Envelope
// {data, status_code, message}. The page merges connectors + toolkits into
// one catalog grid: some connected, some not, one shared-with-me
// (docs-search), the rest created-by-you.

const connector = (over) => ({
  connector_id: over.connector_id,
  provider_type: 'mcp_server',
  owner_id: 'u-001',
  name: over.name,
  url: over.url ?? null,
  transport: 'streamable_http',
  auth_type: over.auth_type ?? 'none',
  url_param_name: null,
  credential_header_name: null,
  description: over.description ?? null,
  display_name: over.display_name ?? null,
  logo_url: null,
  is_active: over.is_active ?? true,
  oauth_configured: over.auth_type === 'oauth2',
  source_kind: over.source_kind ?? 'external',
  build_status: over.build_status ?? null,
  setup_status: over.setup_status ?? 'active',
  setup_error: null,
  created_at: '2026-07-20T09:12:00Z',
  updated_at: '2026-07-29T14:30:00Z',
  is_owner: over.is_owner ?? true,
  version: over.version ?? null,
  tool_count: over.tool_count ?? 0,
  is_connected: over.is_connected ?? false,
  owner_username: over.owner_username ?? 'admin',
});

const createdByYou = [
  connector({
    connector_id: '6f1d2a3b-1111-4a4a-9b9b-000000000001',
    name: 'github',
    display_name: 'GitHub',
    url: 'https://api.githubcopilot.com/mcp',
    auth_type: 'oauth2',
    description: 'Repository, issue, and pull-request tools from the official GitHub MCP server',
    tool_count: 38,
    is_connected: true,
  }),
  connector({
    connector_id: '6f1d2a3b-1111-4a4a-9b9b-000000000002',
    name: 'slack',
    display_name: 'Slack',
    url: 'https://mcp.slack.example.com/mcp',
    auth_type: 'bearer',
    description: 'Post messages, search channels, and manage threads',
    tool_count: 12,
    is_connected: true,
  }),
  connector({
    connector_id: '6f1d2a3b-1111-4a4a-9b9b-000000000003',
    name: 'postgres-tools',
    display_name: 'Postgres Tools',
    url: null,
    auth_type: 'none',
    description: 'Custom SQL query + schema inspection server (uploaded)',
    source_kind: 'uploaded_build',
    build_status: 'building',
    version: 'v2',
    tool_count: 0,
  }),
  connector({
    connector_id: '6f1d2a3b-1111-4a4a-9b9b-000000000004',
    name: 'weather-server',
    display_name: 'Weather Server',
    url: null,
    auth_type: 'none',
    description: 'Forecast lookups from source build',
    source_kind: 'uploaded_build',
    build_status: 'failed',
    version: 'v1',
    tool_count: 0,
    is_active: false,
  }),
];

const sharedWithYou = [
  connector({
    connector_id: '6f1d2a3b-2222-4a4a-9b9b-000000000005',
    name: 'docs-search',
    display_name: 'Docs Search',
    url: 'https://mcp.internal.example.com/docs',
    auth_type: 'url_param',
    description: 'Full-text search over the internal documentation corpus',
    tool_count: 4,
    is_owner: false,
    owner_username: 'priya',
  }),
];

const agentConnectors = {
  connectors: [
    {
      connector_id: '6f1d2a3b-1111-4a4a-9b9b-000000000001',
      provider_type: 'mcp_server',
      name: 'github',
      display_name: 'GitHub',
      description: 'Repository, issue, and pull-request tools from the official GitHub MCP server',
      logo_url: null,
      enabled: true,
      connected: true,
    },
    {
      connector_id: '6f1d2a3b-1111-4a4a-9b9b-000000000002',
      provider_type: 'mcp_server',
      name: 'slack',
      display_name: 'Slack',
      description: 'Post messages, search channels, and manage threads',
      logo_url: null,
      enabled: false,
      connected: true,
    },
    {
      connector_id: '6f1d2a3b-2222-4a4a-9b9b-000000000005',
      provider_type: 'mcp_server',
      name: 'docs-search',
      display_name: 'Docs Search',
      description: 'Full-text search over the internal documentation corpus',
      logo_url: null,
      enabled: true,
      connected: true,
    },
  ],
};

const githubTools = {
  tools: [
    { name: 'create_issue', description: 'Open a new issue in a repository.', stance: 'allow', last_synced_at: '2026-07-30T10:00:00Z' },
    { name: 'get_pull_request', description: 'Fetch a pull request with its metadata and review state.', stance: 'allow', last_synced_at: '2026-07-30T10:00:00Z' },
    { name: 'merge_pull_request', description: 'Merge an open pull request. Requires write access.', stance: 'deny', last_synced_at: '2026-07-30T10:00:00Z' },
    { name: 'search_code', description: 'Search code across repositories with the GitHub code-search syntax.', stance: 'allow', last_synced_at: '2026-07-30T10:00:00Z' },
    { name: 'delete_branch', description: 'Delete a branch from a repository. Destructive.', stance: 'deny', last_synced_at: '2026-07-30T10:00:00Z' },
  ],
};

// Toolkits — GET /api/mcp/composio/toolkits (catalog.rs list_toolkits_view).
// Data-URI logos keep the preview offline; the broken path exercises the
// letter-avatar fallback.
const svgLogo = (fill, glyph) => 'data:image/svg+xml,' + encodeURIComponent(
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect width="24" height="24" rx="5" fill="${fill}"/><text x="12" y="16.5" font-family="Arial" font-size="12" font-weight="bold" fill="#fff" text-anchor="middle">${glyph}</text></svg>`,
);

const toolkit = (over) => ({
  connector_id: over.connector_id,
  name: over.name,
  display_name: over.display_name,
  description: over.description,
  logo_url: over.logo_url ?? null,
  auth_flow: over.auth_flow ?? 'oauth',
  tool_count: over.tool_count ?? 0,
  is_connected: over.is_connected ?? false,
});

const toolkits = [
  toolkit({
    connector_id: '7a2e3b4c-3333-4b4b-8c8c-000000000001',
    name: 'gmail',
    display_name: 'Gmail',
    description: 'Send, search, and label email; manage drafts and threads in the user’s mailbox.',
    logo_url: svgLogo('#ea4335', 'M'),
    tool_count: 24,
    is_connected: true,
  }),
  toolkit({
    connector_id: '7a2e3b4c-3333-4b4b-8c8c-000000000002',
    name: 'figma',
    display_name: 'Figma',
    description: 'Read files, comments, and components; export frames from design documents.',
    logo_url: svgLogo('#0acf83', 'F'),
    tool_count: 18,
  }),
  toolkit({
    connector_id: '7a2e3b4c-3333-4b4b-8c8c-000000000003',
    name: 'outlook',
    display_name: 'Outlook',
    description: 'Mail and calendar tools for Microsoft 365 accounts: send, schedule, and search.',
    logo_url: svgLogo('#0078d4', 'O'),
    tool_count: 31,
  }),
  toolkit({
    connector_id: '7a2e3b4c-3333-4b4b-8c8c-000000000004',
    name: 'notion',
    display_name: 'Notion',
    description: 'Create and update pages and databases; search across the connected workspace.',
    logo_url: svgLogo('#111111', 'N'),
    tool_count: 12,
    is_connected: true,
  }),
  toolkit({
    connector_id: '7a2e3b4c-3333-4b4b-8c8c-000000000005',
    name: 'jira',
    display_name: 'Jira',
    description: 'Project tracking for agile teams: create issues, transition workflows, run JQL searches.',
    logo_url: '/assets/logos/jira-missing.svg', // 404s → letter avatar fallback
    tool_count: 42,
  }),
  toolkit({
    connector_id: '7a2e3b4c-3333-4b4b-8c8c-000000000006',
    name: 'linear',
    display_name: 'Linear',
    description: 'Issue tracking tools: create, assign, and update issues and projects.',
    auth_flow: 'api_key',
    tool_count: 9,
  }),
];

const buildLogs = [
  '#8 [4/6] RUN pip install --no-cache-dir -r requirements.txt',
  '#8 12.41 Collecting mcp>=1.2.0',
  '#8 14.02 Installing collected packages: mcp, httpx, pydantic',
  '#8 18.77 Successfully installed httpx-0.28.1 mcp-1.9.0 pydantic-2.11.3',
  '#8 DONE 19.1s',
  '#9 [5/6] COPY . /app',
  '#9 DONE 0.2s',
  '#10 [6/6] CMD ["python", "server.py"]',
  '#10 DONE 0.0s',
  'build: image tagged mcp-postgres-tools:v2',
  'deploy: waiting for container health check…',
].join('\n');

export default {
  fetch: [
    ['GET /api/mcp/connectors', {
      data: {
        created_by_you: createdByYou,
        shared_with_you: sharedWithYou,
        total: createdByYou.length + sharedWithYou.length,
      },
      status_code: 200,
      message: 'Connectors retrieved successfully',
    }],

    ['GET /api/mcp/connectors/my-uploads', {
      data: [
        {
          connector_id: '6f1d2a3b-1111-4a4a-9b9b-000000000003',
          connector_name: 'postgres-tools',
          icon_url: null,
          upload_info: {
            upload_type: 'mcp_server',
            upload_status: 'Deploying',
            status_message: 'MCP server is being built...',
            error_detail: null,
          },
          url: null,
          description: null,
        },
        {
          connector_id: '6f1d2a3b-1111-4a4a-9b9b-000000000004',
          connector_name: 'weather-server',
          icon_url: null,
          upload_info: {
            upload_type: 'mcp_server',
            upload_status: 'Failed',
            status_message: 'Build failed',
            error_detail: 'Dockerfile step 4/6 failed: requirements.txt not found',
          },
          url: null,
          description: null,
        },
      ],
      status_code: 200,
      message: 'Retrieved 2 uploaded MCP connectors',
    }],

    // fetchAgents → GET /api/agents?q=&page=1&limit=100
    [{ method: 'GET', path: /^\/api\/agents(\?|$)/ }, {
      data: [
        { id: 'a-001', name: 'coding-agent', display_name: 'Coding Agent', status: 'running' },
        { id: 'a-002', name: 'research-agent', display_name: 'Research Agent', status: 'running' },
        { id: 'a-004', name: 'qa-agent', display_name: 'QA Agent', status: 'running' },
      ],
      total: 3,
    }],

    ['POST /api/mcp/connectors/probe', {
      data: {
        url: 'https://mcp.example.com/mcp',
        auth_type: 'oauth2',
        requires: 'oauth_flow',
        supports_dcr: true,
        hint: 'This server supports OAuth 2.1 with automatic client registration — no client credentials needed.',
      },
      status_code: 200,
      message: 'Connector probed successfully',
    }],

    ['POST /api/mcp/connectors', {
      data: connector({
        connector_id: '6f1d2a3b-1111-4a4a-9b9b-00000000000a',
        name: 'new-connector',
        url: 'https://mcp.example.com/mcp',
        auth_type: 'oauth2',
      }),
      status_code: 201,
      message: 'Connector created successfully',
    }],

    ['POST /api/mcp/connectors/upload-github', {
      data: {
        connector_id: '6f1d2a3b-1111-4a4a-9b9b-00000000000b',
        build_id: '9c8d7e6f-3333-4b4b-8c8c-000000000001',
      },
      status_code: 202,
      message: 'MCP server build queued',
    }],
    ['POST /api/mcp/connectors/upload', {
      data: {
        connector_id: '6f1d2a3b-1111-4a4a-9b9b-00000000000c',
        build_id: '9c8d7e6f-3333-4b4b-8c8c-000000000002',
      },
      status_code: 202,
      message: 'MCP server build queued',
    }],

    [{ method: 'GET', path: /^\/api\/mcp\/connectors\/[0-9a-f-]+\/build-logs/ }, {
      data: buildLogs,
      status_code: 200,
      message: 'build logs retrieved successfully',
    }],
    [{ method: 'GET', path: /^\/api\/mcp\/connectors\/[0-9a-f-]+\/build-status$/ }, {
      data: { build_status: 'building', error_msg: null, image_tag: 'mcp-postgres-tools:v2' },
      status_code: 200,
      message: 'build status retrieved successfully',
    }],

    [{ method: 'GET', path: /^\/api\/mcp\/connectors\/[0-9a-f-]+\/credential\/status$/ }, {
      data: {
        connector_id: '6f1d2a3b-1111-4a4a-9b9b-000000000002',
        name: 'slack',
        connected: true,
        auth_type: 'bearer',
      },
      status_code: 200,
      message: 'Credential status retrieved successfully',
    }],
    [{ method: 'POST', path: /^\/api\/mcp\/connectors\/[0-9a-f-]+\/credential$/ }, {
      data: {
        connector_id: '6f1d2a3b-1111-4a4a-9b9b-000000000002',
        name: 'slack',
        connected: true,
        error: null,
      },
      status_code: 201,
      message: 'Credential registered and verified successfully',
    }],
    [{ method: 'DELETE', path: /^\/api\/mcp\/connectors\/[0-9a-f-]+\/credential$/ }, {
      data: null, status_code: 200, message: 'Credential deleted successfully',
    }],

    [{ method: 'GET', path: /^\/api\/mcp\/connectors\/[0-9a-f-]+\/oauth\/status$/ }, {
      data: {
        connector_id: '6f1d2a3b-1111-4a4a-9b9b-000000000001',
        name: 'github',
        authorized: true,
        expires_at: '2026-08-15T09:00:00Z',
        scope: 'repo read:org',
      },
      status_code: 200,
      message: 'OAuth status retrieved successfully',
    }],
    [{ method: 'POST', path: /^\/api\/mcp\/connectors\/[0-9a-f-]+\/oauth\/authorize$/ }, {
      data: {
        connector_id: '6f1d2a3b-1111-4a4a-9b9b-000000000001',
        name: 'github',
        authorization_url: 'https://github.com/login/oauth/authorize?client_id=abc',
      },
      status_code: 200,
      message: 'OAuth authorization URL generated successfully',
    }],
    [{ method: 'DELETE', path: /^\/api\/mcp\/connectors\/[0-9a-f-]+\/oauth\/token$/ }, {
      data: null, status_code: 200, message: 'OAuth token revoked successfully',
    }],

    [{ method: 'DELETE', path: /^\/api\/mcp\/connectors\/[0-9a-f-]+$/ }, {
      data: null, status_code: 200, message: 'Connector deleted successfully',
    }],

    ['GET /api/mcp/composio/toolkits', {
      data: { toolkits, total: toolkits.length },
      status_code: 200,
      message: 'Toolkits retrieved successfully',
    }],
    ['POST /api/mcp/connect', {
      data: {
        status: 'initiated',
        connector_id: '7a2e3b4c-3333-4b4b-8c8c-000000000002',
        name: 'figma',
        oauth_url: 'https://backend.composio.dev/authorize?state=abc',
      },
      status_code: 201,
      message: 'OAuth flow initiated',
    }],
    ['GET /api/mcp/connections', {
      data: {
        connections: [
          { connector_id: '7a2e3b4c-3333-4b4b-8c8c-000000000001', name: 'gmail', status: 'ACTIVE' },
          { connector_id: '7a2e3b4c-3333-4b4b-8c8c-000000000004', name: 'notion', status: 'ACTIVE' },
        ],
      },
      status_code: 200,
      message: 'Connections retrieved successfully',
    }],
    [{ method: 'DELETE', path: /^\/api\/mcp\/connections\/[0-9a-f-]+$/ }, {
      data: {
        message: 'Disconnected',
        connector_id: '7a2e3b4c-3333-4b4b-8c8c-000000000001',
        composio_revoked: true,
      },
      status_code: 200,
      message: 'Disconnected successfully',
    }],

    [{ method: 'GET', path: /^\/api\/mcp\/agents\/[^/]+\/connectors$/ }, {
      data: agentConnectors,
      status_code: 200,
      message: 'Agent connectors retrieved successfully',
    }],
    [{ method: 'PUT', path: /^\/api\/mcp\/agents\/[^/]+\/connectors\/[0-9a-f-]+$/ }, {
      data: { connector_id: '6f1d2a3b-1111-4a4a-9b9b-000000000002', enabled: true },
      status_code: 200,
      message: 'Connector access updated successfully',
    }],
    [{ method: 'GET', path: /^\/api\/mcp\/agents\/[^/]+\/connectors\/[0-9a-f-]+\/tools$/ }, {
      data: githubTools,
      status_code: 200,
      message: 'Connector tools retrieved successfully',
    }],
    [{ method: 'GET', path: /^\/api\/mcp\/agents\/[^/]+\/tools$/ }, {
      data: {
        rules: [
          { connector_id: '6f1d2a3b-1111-4a4a-9b9b-000000000001', tool_pattern: 'merge_pull_request', stance: 'deny' },
          { connector_id: '6f1d2a3b-1111-4a4a-9b9b-000000000001', tool_pattern: 'delete_branch', stance: 'deny' },
        ],
      },
      status_code: 200,
      message: 'Tool rules retrieved successfully',
    }],
    [{ method: 'PUT', path: /^\/api\/mcp\/agents\/[^/]+\/tools$/ }, {
      data: { rules: [] },
      status_code: 200,
      message: 'Tool rules updated successfully',
    }],
  ],

  scenarios: {
    "rail-expanded": async (page) => {
      await page.evaluate(() => document.querySelector("[data-rail-toggle]")?.click());
      await new Promise((r) => setTimeout(r, 600));
    },
    'register-open': async (page) => {
      await page.click('#register-btn');
      await page.waitForSelector('#register-modal dialog[open]');
      await page.fill('#register-form input[name="url"]', 'https://mcp.example.com/mcp');
      await page.click('#probe-btn');
      await page.waitForSelector('#probe-result.is-ok');
    },
    'upload-open': async (page) => {
      await page.click('#upload-btn');
      await page.waitForSelector('#upload-modal dialog[open]');
    },
    // Unified catalog tabs — counts come from the merged toolkit+server set.
    'available-tab': async (page) => {
      await page.waitForSelector('.tk-card[data-id]');
      await page.click('.tk-tab[data-tab="available"]');
      await new Promise((r) => setTimeout(r, 300));
    },
    'connected-tab': async (page) => {
      await page.waitForSelector('.tk-card[data-id]');
      await page.click('.tk-tab[data-tab="connected"]');
      await new Promise((r) => setTimeout(r, 300));
    },
    // Nav scopes — ownership scopes show custom servers only; the toolkits
    // scope shows Composio cards only.
    'my-servers': async (page) => {
      await page.waitForSelector('.tk-card[data-id]');
      await page.click('app-module-nav [data-section="my-servers"]');
      await new Promise((r) => setTimeout(r, 400));
    },
    'toolkits-filter': async (page) => {
      await page.waitForSelector('.tk-card[data-id]');
      await page.click('app-module-nav [data-section="toolkits"]');
      await new Promise((r) => setTimeout(r, 400));
    },
    'connector-detail': async (page) => {
      // GitHub is a custom OAuth server — card body click opens the detail
      // modal with meta + OAuth status + owner delete.
      await page.waitForSelector('.tk-card[data-id]');
      await page.click('.tk-card[data-id="6f1d2a3b-1111-4a4a-9b9b-000000000001"] .tk-name');
      await page.waitForSelector('#detail-modal dialog[open]');
      await page.waitForSelector('#oauth-revoke');
    },
    'catalog-empty': async (page) => {
      await page.evaluate(() => {
        window.fetchMcpConnectors = async () => (
          { data: { created_by_you: [], shared_with_you: [], total: 0 }, status_code: 200 });
        window.fetchMcpToolkits = async () => ({ data: { toolkits: [], total: 0 }, status_code: 200 });
        window.fetchMcpMyUploads = async () => ({ data: [], status_code: 200 });
        document.querySelector('mcp-page').remove();
        document.body.appendChild(document.createElement('mcp-page'));
      });
      await page.waitForSelector('#catalog-grid .empty-state');
    },
    'agent-access-in-detail': async (page) => {
      // Open a connector detail modal, then use the agent picker inside it.
      await page.waitForSelector('.tk-card[data-id]');
      await page.click('.tk-card[data-id="6f1d2a3b-1111-4a4a-9b9b-000000000001"] .tk-name');
      await page.waitForSelector('#detail-modal dialog[open]');
      await page.click('#detail-agent-select .ac-input');
      await page.fill('#detail-agent-select .ac-input', 'coding');
      await page.waitForSelector('#detail-agent-select .ac-option');
      await page.click('#detail-agent-select .ac-option');
      await new Promise((r) => setTimeout(r, 300));
    },
    'build-logs': async (page) => {
      await page.click('app-module-nav [data-section="my-servers"]');
      await new Promise((r) => setTimeout(r, 400));
      await page.click('#uploads-tbody .act-logs');
      await page.waitForSelector('#logs-panel:not([hidden])');
      await page.evaluate(() => document.querySelector('#uploads-inline')?.scrollIntoView());
      await new Promise((r) => setTimeout(r, 300));
    },
  },
};
