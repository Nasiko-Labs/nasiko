/**
 * The MCP mock (plans/feat-mcp.md §8), asserted through the MSW handlers: its ids and rows come from the same seed
 * `seed:live` writes, so recorded fixtures replay against it, and it follows the server's build rules.
 */
import { afterEach, describe, expect, it } from 'vitest'
import { apiData } from '@/lib/api/client'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { configureMocks, mcpMockState } from './handlers'
import { MCP_ACCESS, MCP_CONNECTORS, mcpConnectorId } from './seed-mcp'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, variant: null }))

type List = { created_by_you: { connector_id: string; name: string }[] }

describe('MCP mock', () => {
  it('uses the shared seed’s ids, and its access rows sit on the live seed’s first agents', () => {
    const st = mcpMockState()
    for (const c of MCP_CONNECTORS)
      expect(st.connectors.find((x) => x.name === c.name)?.id).toBe(mcpConnectorId(c.n))
    const live = seed.agents.filter((a) => !a.deleted)
    for (const a of MCP_ACCESS) {
      const c = MCP_CONNECTORS.find((x) => x.name === a.name)!
      expect(st.access.get(`${live[a.agent]!.id}:${mcpConnectorId(c.n)}`)?.enabled).toBe(a.enabled)
    }
  })

  it('drops a first upload whose build fails, as build.rs does, and keeps a failed re-upload', async () => {
    configureMocks({ variant: 'mcp-upload-fails' })
    const up = await apiData<{ connector_id: string }>('/api/mcp/connectors/upload-github', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name: 'doomed', github_url: 'https://github.com/x/y' }),
    })
    const list = () => apiData<List>('/api/mcp/connectors')
    expect((await list()).created_by_you.map((c) => c.connector_id)).toContain(up.connector_id)
    // The build ends.
    const b = mcpMockState().connectors.find((c) => c.id === up.connector_id)!.build!
    b.doneAt = b.startedAt = now() - 60_000
    const names = (await list()).created_by_you.map((c) => c.name)
    expect(names).not.toContain('doomed')
    expect(names).toContain('sql-runner')
  })
})
