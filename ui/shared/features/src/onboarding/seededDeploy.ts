/**
 * Deploys the guide's prebuilt agents through the Registry tab's own import (`POST /api/import/registry`), one request
 * per agent, not awaited: the guide moves on while they import (usually 1–5 min each). Each outcome is a toast, as an
 * import finished away from the Registry tab is, and refreshes the agents list so a later visit sees them as deployed.
 */
import type { QueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { agentKeys } from '@nasiko/features/agents/api'
import { sendRegistryImport } from '@nasiko/features/deploy/api'
import { importFinished } from '@nasiko/features/deploy/follower'
import { copy } from './copy'
import type { SeededAgent } from './logic'

export function deploySeeded(qc: QueryClient, agents: readonly SeededAgent[]): void {
  for (const a of agents) {
    sendRegistryImport(a.reference).then(
      (r) => {
        void qc.invalidateQueries({ queryKey: agentKeys.directory })
        importFinished(
          r.container_name
            ? { kind: 'running', buildId: r.build_id ?? null, agentId: r.agent_id, name: a.name }
            : { kind: 'notRunning', agentId: r.agent_id, name: a.name },
        )
      },
      (e: unknown) => {
        // The row may exist even when the deploy failed: refresh, so a retry doesn't import it twice.
        void qc.invalidateQueries({ queryKey: agentKeys.directory })
        toast.error(copy.agent.deployFailed(a.title, e instanceof Error ? e.message : String(e)))
      },
    )
  }
}
