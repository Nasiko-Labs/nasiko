// @vitest-environment node
import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { activeItem, backIndex, NAV_GROUPS, NAV_ITEMS } from './nav'

/** The header nav before the sidebar (src/routes/__root.tsx at v0.7.1.0), minus Status (now the footer row) and Weave (EE only). */
const OLD_NAV = [
  { to: '/tokenops', label: 'TokenOps', shared: true },
  { to: '/sessions', label: 'Sessions', shared: true },
  { to: '/harnesses', label: 'Harnesses', shared: true },
  { to: '/agents', label: 'Agents', shared: false },
  { to: '/chat', label: 'Chat', shared: false },
]

describe('nav items', () => {
  it('keeps every old destination, label and shared flag (eng D5)', () => {
    const now = NAV_ITEMS.map(({ to, label, shared }) => ({ to, label, shared }))
    for (const old of OLD_NAV) expect(now).toContainEqual(old)
    // Added since: the Overview (plans/feat-overview.md §3), Workflows (plans/feat-workflows.md §1), the LLM router
    // (plans/feat-llm-router.md §4) and MCP servers (plans/feat-mcp.md §1). Deploy and Builds are Agents' sub-pages and
    // Settings is a footer row (sidebar review 2026-10-01).
    expect(now.filter((n) => !OLD_NAV.some((o) => o.to === n.to))).toEqual([
      { to: '/', label: 'Overview', shared: false },
      { to: '/workflows', label: 'Workflows', shared: false },
      { to: '/router', label: 'LLM router', shared: false },
      { to: '/mcp', label: 'MCP servers', shared: false },
      { to: '/compact-tools', label: 'Compact Tools', shared: false },
    ])
  })

  it('lists groups by job, each item in a known group, most-used first', () => {
    expect(NAV_GROUPS.map((g) => [g.id, g.label])).toEqual([
      ['work', null],
      ['fleet', 'Fleet'],
      ['connect', 'Connect'],
      ['observe', 'Observe'],
      ['lab', 'Lab'],
    ])
    expect(NAV_ITEMS.map((n) => n.label)).toEqual([
      'Overview',
      'Chat',
      'Agents',
      'Workflows',
      'LLM router',
      'MCP servers',
      'Compact Tools',
      'Sessions',
      'TokenOps',
      'Harnesses',
    ])
    // Items of one group are contiguous and follow the group order.
    const order = NAV_ITEMS.map((n) => NAV_GROUPS.findIndex((g) => g.id === n.group))
    expect(order).toEqual([...order].sort())
  })

  it('points every item at a route that exists, inside the signed-in shell', () => {
    const gen = readFileSync(
      new URL('../../../../oss/src/routeTree.gen.ts', import.meta.url),
      'utf8',
    )
    const byTo = gen.slice(
      gen.indexOf('export interface FileRoutesByTo'),
      gen.indexOf('export interface FileRoutesById'),
    )
    for (const n of NAV_ITEMS) expect(byTo, n.to).toContain(`'${n.to}':`)
    // Status moved under _app (eng D9, design 3B), so the sidebar wraps it too.
    expect(gen).toMatch(
      /AppIndexRoute = AppIndexRouteImport\.update\(\{[^}]*getParentRoute: \(\) => AppRoute/,
    )
    expect(gen).toMatch(
      /AppStatusRoute = AppStatusRouteImport\.update\(\{[^}]*getParentRoute: \(\) => AppRoute/,
    )
    // Weave is EE only: the OSS route tree has no /weave.
    expect(gen).not.toContain("'/weave'")
  })

  it('marks the item for sub-routes and owned pages, Overview only on / itself, and none on Status', () => {
    expect(activeItem('/agents/mine')?.label).toBe('Agents')
    expect(activeItem('/agents/5eed0000-0000-4000-8000-000000000001')?.label).toBe('Agents')
    expect(activeItem('/sessions/abc')?.label).toBe('Sessions')
    expect(activeItem('/chat/abc')?.label).toBe('Chat')
    expect(activeItem('/tokenops')?.label).toBe('TokenOps')
    expect(activeItem('/compact-tools')?.label).toBe('Compact Tools')
    expect(activeItem('/')?.label).toBe('Overview')
    expect(activeItem('/status')).toBeUndefined()
    expect(activeItem('/chatter')).toBeUndefined()
    // Agents owns Deploy and Builds (its sub-nav); Settings is the footer's row, not a nav item.
    expect(activeItem('/deploy')?.label).toBe('Agents')
    expect(activeItem('/builds')?.label).toBe('Agents')
    expect(activeItem('/builds/5eed000b-0000-4000-8000-000000000001')?.label).toBe('Agents')
    expect(activeItem('/buildsx')).toBeUndefined()
    expect(activeItem('/settings/secrets')).toBeUndefined()
  })

  it('Back from a panel goes to the newest page outside its module, search and all', () => {
    const trail = ['/agents', '/tokenops?preset=7d', '/chat', '/chat/abc', '/chat/def']
    expect(backIndex(trail, '/chat/def')).toBe(1)
    const settings = ['/', '/settings', '/settings?section=limits', '/settings/secrets']
    expect(backIndex(settings, '/settings/secrets')).toBe(0)
    // Chat ↔ Settings are different modules.
    expect(backIndex(['/settings', '/chat'], '/chat')).toBe(0)
    // Opened directly: nothing to go back to.
    expect(backIndex(['/chat/abc', '/chat'], '/chat')).toBe(-1)
    expect(backIndex([], '/chat')).toBe(-1)
  })
})
