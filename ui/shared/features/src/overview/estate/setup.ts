/**
 * The canvases' setup steps: what the estate still lacks, read from the estate itself, so the Map's empty cards and
 * Kiko's checklist on the village name the same steps, in the same order, and go to the same pages. A step is done
 * once the estate shows its part: a run or spend in the range, a coding-harness install, a tool, a router that isn't
 * the direct path, a workflow. Setup is the workspace's, so Mine reads the same steps as Org.
 */
import type { Estate } from './types'

export type StepId = 'chat' | 'harness' | 'mcp' | 'router' | 'workflows'
export type StepTo = '/chat' | '/harnesses' | '/mcp' | '/router' | '/workflows/new'
export interface Step {
  id: StepId
  done: boolean
  to: StepTo
}

/** Where each step is set up. */
export const STEP_TO: Record<StepId, StepTo> = {
  chat: '/chat',
  harness: '/harnesses',
  mcp: '/mcp',
  router: '/router',
  workflows: '/workflows/new',
}

/** Nothing in the range called a model: no agent ran or spent, no coding harness spent or held a session. */
export function nothingRan(e: Estate): boolean {
  return (
    e.agents.every((a) => !a.runs && !a.spend_usd) &&
    e.harnesses.every((h) => !h.spend_usd && !h.sessions.length)
  )
}

export function setupSteps(e: Estate): Step[] {
  const done: Record<StepId, boolean> = {
    chat: !nothingRan(e),
    harness: e.harnesses.length > 0,
    mcp: e.tools.length > 0,
    router: e.routers.some((r) => !r.direct),
    workflows: e.workflows.length > 0,
  }
  return (Object.keys(STEP_TO) as StepId[]).map((id) => ({ id, done: done[id], to: STEP_TO[id] }))
}
