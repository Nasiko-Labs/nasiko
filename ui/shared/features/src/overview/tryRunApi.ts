/**
 * The try-a-run card's write (components/TryAgentRun.tsx): Token optimization switched on with the agent Settings
 * switch's own request (`PUT /api/agents/{id}`, `compress_enabled`). The run itself is chat/runOnce.ts.
 */
import { apiFetch } from '@nasiko/data-access/client'

/** Token optimization on for `agentId`: only `compress_enabled` is sent (the update is COALESCE per field). */
export const enableOptimization = (agentId: string) =>
  apiFetch(`/api/agents/${encodeURIComponent(agentId)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ compress_enabled: true }),
  })
