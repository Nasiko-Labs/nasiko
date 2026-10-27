/** Each map node's owning page, for the insight panel's "Open in …" (pure). */
import type { EstateNode } from '../estate/model'
import type { Estate, Target } from '../estate/types'

/** A node's owning page, with its name for "Open in …"; null when it has none (people, teams). */
export function homeOf(e: Estate, n: EstateNode): { target: Target; page: string } | null {
  switch (n.kind) {
    case 'agent':
      return { target: { page: 'agent', id: n.id }, page: 'Agents' }
    case 'harness':
      return { target: { page: 'harnesses' }, page: 'Coding harnesses' }
    case 'tool':
      return { target: { page: 'connector', id: n.id }, page: 'MCP servers' }
    case 'router':
    case 'model':
    case 'provider':
      return { target: { page: 'router' }, page: 'LLM router' }
    case 'workflow':
      return e.workflows.some((w) => w.id === n.id) && !n.id.startsWith('wf-')
        ? { target: { page: 'workflow', id: n.id }, page: 'Workflows' }
        : null
    default:
      return null
  }
}
