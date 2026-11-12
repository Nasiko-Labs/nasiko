/**
 * Deploys the guide's prebuilt agents through the Registry tab's own import (`POST /api/import/registry`), one request
 * per agent, not awaited: the guide moves on while they import (usually 1–5 min each). Each one's progress goes to the
 * guide's deploy store (api.ts), which the guide and Home show (NAS-1450). The start is a toast, and so is each outcome,
 * as an import finished away from the Registry tab is; an outcome also refreshes the agents list so a later visit sees
 * them as deployed.
 */
import type { QueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { ApiError } from '@nasiko/data-access/client'
import { agentKeys } from '@nasiko/features/agents/api'
import { sendRegistryImport } from '@nasiko/features/deploy/api'
import { importFinished } from '@nasiko/features/deploy/follower'
import { type SeededDeploy, seededDeployOf, setSeededDeploy } from './api'
import { copy } from './copy'
import type { SeededAgent } from './logic'

export function deploySeeded(qc: QueryClient, agents: readonly SeededAgent[]): void {
  // One toast for the lot, as they start; each one's finish has its own.
  toast(copy.agent.deployStarted(agents.map((a) => a.title)))
  for (const a of agents) {
    const started: SeededDeploy = { phase: 'deploying', title: a.title, startedAt: Date.now() }
    setSeededDeploy(a.name, started)
    // Cleared meanwhile (sign-out): neither written back nor announced to the next account.
    const current = () => seededDeployOf(a.name) === started
    sendRegistryImport(a.reference).then(
      (r) => {
        void qc.invalidateQueries({ queryKey: agentKeys.directory })
        if (!current()) return
        const running = !!r.container_name
        setSeededDeploy(a.name, {
          phase: running ? 'running' : 'notRunning',
          title: a.title,
          agentId: r.agent_id,
        })
        importFinished(
          running
            ? { kind: 'running', buildId: r.build_id ?? null, agentId: r.agent_id, name: a.name }
            : { kind: 'notRunning', agentId: r.agent_id, name: a.name },
        )
      },
      (e: unknown) => {
        // The row may exist even when the deploy failed: refresh, so a retry doesn't import it twice.
        void qc.invalidateQueries({ queryKey: agentKeys.directory })
        if (!current()) return
        // The server's plain-text reason when it sent one, as the Registry tab shows it.
        const reason =
          e instanceof ApiError && typeof e.body === 'string' && e.body
            ? e.body
            : e instanceof Error
              ? e.message
              : String(e)
        setSeededDeploy(a.name, { phase: 'failed', title: a.title, reason })
        toast.error(copy.agent.deployFailed(a.title, reason))
      },
    )
  }
}
