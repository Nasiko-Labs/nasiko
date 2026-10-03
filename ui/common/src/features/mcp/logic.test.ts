import { describe, expect, it } from 'vitest'
import {
  authFlowOf,
  grantKind,
  inTab,
  inView,
  isBuilding,
  matchesQuery,
  nameFromFile,
  registerBody,
  rulesWith,
  safePopupUrl,
  serverStatus,
  stanceCounts,
  tabCounts,
  toServices,
  unusable,
  type RegisterValues,
} from './logic'
import type { Connector, ConnectorList, Toolkit } from './types'

const server = (over: Partial<Connector>): Connector => ({
  connector_id: 'c1',
  name: 'docs',
  display_name: null,
  is_active: true,
  is_owner: true,
  tool_count: 2,
  is_connected: false,
  auth_type: 'none',
  source_kind: 'external_url',
  build_status: null,
  ...over,
})
const toolkit = (over: Partial<Toolkit>): Toolkit => ({
  connector_id: 't1',
  name: 'gmail',
  display_name: 'Gmail',
  auth_flow: 'oauth',
  tool_count: 3,
  is_connected: true,
  ...over,
})

describe('MCP logic (plans/feat-mcp.md)', () => {
  it('maps a custom server auth type onto the connect flow (catalog.rs auth_flow_for)', () => {
    expect(authFlowOf('oauth2')).toBe('oauth')
    for (const t of ['bearer', 'basic', 'url_param']) expect(authFlowOf(t)).toBe('api_key')
    expect(authFlowOf('none')).toBe('none')
    expect(authFlowOf(null)).toBe('none')
  })

  it('reads build status only for uploads', () => {
    expect(serverStatus(server({ source_kind: 'uploaded_build', build_status: 'pending' }))).toBe(
      'building',
    )
    expect(serverStatus(server({ source_kind: 'uploaded_build', build_status: 'building' }))).toBe(
      'building',
    )
    expect(serverStatus(server({ source_kind: 'uploaded_build', build_status: 'failed' }))).toBe(
      'failed',
    )
    expect(
      serverStatus(
        server({ source_kind: 'uploaded_build', build_status: 'running', is_active: true }),
      ),
    ).toBe('active')
    // A registered server never carries a build state, whatever the column says.
    expect(serverStatus(server({ build_status: 'failed', is_active: false }))).toBe('inactive')
  })

  it('merges toolkits and servers into one list sorted by display name, marking shared ones', () => {
    const list: ConnectorList = {
      created_by_you: [server({ connector_id: 'a', name: 'zeta' })],
      shared_with_you: [server({ connector_id: 'b', name: 'alpha', owner_username: 'alice' })],
    }
    const all = toServices(list, [toolkit({})])
    expect(all.map((s) => s.label)).toEqual(['alpha', 'Gmail', 'zeta'])
    expect(all.find((s) => s.id === 'b')).toMatchObject({ shared: true, owner: 'alice' })
    expect(all.find((s) => s.id === 't1')).toMatchObject({ kind: 'toolkit', status: null })
  })

  it('scopes, tabs and search filter the way the legacy nav did', () => {
    const all = toServices(
      {
        created_by_you: [
          server({ connector_id: 'a', is_connected: true, description: 'Search docs' }),
        ],
        shared_with_you: [server({ connector_id: 'b', name: 'ledger' })],
      },
      [toolkit({ is_connected: false })],
    )
    expect(all.filter((s) => inView(s, 'yours')).map((s) => s.id)).toEqual(['a'])
    expect(all.filter((s) => inView(s, 'shared')).map((s) => s.id)).toEqual(['b'])
    expect(all.filter((s) => inView(s, 'toolkits')).map((s) => s.id)).toEqual(['t1'])
    expect(tabCounts(all)).toEqual({ all: 3, available: 2, connected: 1 })
    expect(all.filter((s) => inTab(s, 'connected')).map((s) => s.id)).toEqual(['a'])
    expect(all.filter((s) => matchesQuery(s, '  SEARCH ')).map((s) => s.id)).toEqual(['a'])
  })

  it('polls only while an upload is pending or building', () => {
    expect(isBuilding(undefined)).toBe(false)
    const l = (build_status: string): ConnectorList => ({
      created_by_you: [server({ source_kind: 'uploaded_build', build_status })],
      shared_with_you: [],
    })
    expect(isBuilding(l('pending'))).toBe(true)
    expect(isBuilding(l('running'))).toBe(false)
  })

  it('lists uploads agents cannot use yet, by id', () => {
    const list: ConnectorList = {
      created_by_you: [
        server({ connector_id: 'b', source_kind: 'uploaded_build', build_status: 'building' }),
        server({ connector_id: 'f', source_kind: 'uploaded_build', build_status: 'failed' }),
        server({ connector_id: 'r', source_kind: 'uploaded_build', build_status: 'running' }),
        server({ connector_id: 'x' }),
      ],
      shared_with_you: [],
    }
    expect([...unusable(list)]).toEqual([
      ['b', 'building'],
      ['f', 'failed'],
    ])
    expect(unusable(undefined).size).toBe(0)
  })

  it('names an upload from its file the way the legacy dialog does', () => {
    expect(nameFromFile('My MCP Server (v2).zip')).toBe('my-mcp-server-v2')
    expect(nameFromFile('--weird__name.ZIP')).toBe('weird__name')
    expect(nameFromFile(`${'a'.repeat(200)}.zip`)).toHaveLength(128)
  })

  it('sends only the fields of the chosen auth type, and only non-empty ones', () => {
    const v: RegisterValues = {
      name: ' gh ',
      display_name: '',
      url: ' https://mcp.example.com/mcp ',
      auth_type: 'basic',
      credential_header_name: 'X-Key',
      basic_username: 'bot',
      basic_password: 'pw',
      oauth_client_id: 'ignored',
      oauth_client_secret: '',
      url_param_name: '',
      description: 'Tools',
    }
    expect(registerBody(v)).toEqual({
      name: 'gh',
      url: 'https://mcp.example.com/mcp',
      auth_type: 'basic',
      description: 'Tools',
      basic_username: 'bot',
      basic_password: 'pw',
    })
  })

  it('re-sends the whole rule set with one stance changed (PUT /tools replaces per connector)', () => {
    const tools = [
      { name: 'a', stance: 'allow' as const },
      { name: 'b', stance: 'ask' as const },
    ]
    expect(rulesWith('c1', tools, 'a', 'block')).toEqual([
      { connector_id: 'c1', tool_pattern: 'a', stance: 'block' },
      { connector_id: 'c1', tool_pattern: 'b', stance: 'ask' },
    ])
    expect(stanceCounts(tools)).toEqual({ total: 2, allowed: 1, asks: 1 })
  })

  it('labels access reasons, folding every org path into Inherited', () => {
    expect(['owner', 'direct', 'public', 'team', 'department', 'org_unit'].map(grantKind)).toEqual([
      'owner',
      'direct',
      'public',
      'inherited',
      'inherited',
      'inherited',
    ])
  })

  it('opens OAuth pages over https (or the mock about:) only', () => {
    expect(safePopupUrl('https://auth.example.com/authorize?x=1')).toBe(
      'https://auth.example.com/authorize?x=1',
    )
    expect(safePopupUrl('about:blank#mcp-oauth')).toBe('about:blank#mcp-oauth')
    expect(safePopupUrl('http://auth.example.com')).toBeNull()
    expect(safePopupUrl('javascript:alert(1)')).toBeNull()
    expect(safePopupUrl(undefined)).toBeNull()
    expect(safePopupUrl('not a url')).toBeNull()
  })
})
