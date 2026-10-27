/**
 * A flow's kind (F8, eng review A3), from the fields each writer sets at `6ab60326`: the dispatcher's root agent is
 * `orchestrator` (`router/a2a_dispatch.rs`), MAF writes `metadata.mode = "free_flowing"` (`maf/executor.rs`), every
 * other flow is a direct call through the proxy. A "HITL resume" flow takes the kind of its root agent.
 */
import { metaString, type Flow } from './types'

export type FlowKind = 'orchestrated' | 'direct' | 'workflow'

const ORCHESTRATOR = 'orchestrator'

export function flowKind(flow: Pick<Flow, 'root_agent_name' | 'metadata'>): FlowKind {
  if (flow.root_agent_name === ORCHESTRATOR) return 'orchestrated'
  if (metaString(flow, 'mode') === 'free_flowing') return 'workflow'
  return 'direct'
}
