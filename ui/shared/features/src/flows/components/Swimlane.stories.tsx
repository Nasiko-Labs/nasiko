// The flow swimlane (plans/feat-flows.md T2, F10, F21-F23): three flows built through the page's own pipeline
// (lanes, axis, fan-outs, critical path), so axe checks what the flow page draws: a routed fan-out, a running call and
// a paused flow whose wait collapses into a break.
import type { Meta, StoryObj } from '@storybook/react-vite'
import { useState } from 'react'
import type { Call } from '../calls'
import { criticalPath } from '../critical'
import { buildLanes } from '../lanes'
import { buildAxis, fanOuts } from '../timeline'
import { Swimlane } from './Swimlane'

const meta: Meta = { title: 'Flows/Swimlane' }
export default meta

const T0 = Date.UTC(2026, 9, 6, 10)
const SEC = 1_000

const call = (
  key: string,
  agent: string,
  from: number,
  to: number | null,
  over: Partial<Call> = {},
): Call => ({
  key: `step:${key}`,
  agentId: agent,
  agentName: agent,
  startMs: T0 + from * SEC,
  endMs: to === null ? null : T0 + to * SEC,
  status: to === null ? 'running' : 'completed',
  source: 'recorded',
  stepId: key,
  spanId: null,
  parentKey: null,
  input: null,
  output: null,
  error: null,
  tokens: null,
  exactEnd: true,
  maybeSame: false,
  waitStartMs: null,
  ...over,
})

function Flow({
  calls,
  endS,
  nowS,
  routed = true,
  label,
}: {
  calls: Call[]
  endS: number | null
  nowS: number
  routed?: boolean
  label: string
}) {
  const [selected, setSelected] = useState<string | null>(null)
  const now = T0 + nowS * SEC
  const endMs = endS === null ? null : T0 + endS * SEC
  const path = criticalPath(calls)
  return (
    <div className="max-w-3xl">
      <Swimlane
        model={buildLanes(calls)}
        axis={buildAxis(calls, T0, endMs, now)}
        now={now}
        orchestrator={routed ? { startMs: T0, endMs } : null}
        fanOuts={fanOuts(calls, now)}
        critical={path.keys}
        label={label}
        selected={selected}
        onSelect={setSelected}
      />
    </div>
  )
}

export const RoutedFanOut: StoryObj = {
  render: () => (
    <Flow
      label="4 calls across 3 agents. Took 9.0 s."
      endS={9}
      nowS={9}
      calls={[
        call('1', 'triage', 0.2, 1.4),
        call('2', 'billing', 1.6, 6.8),
        call('3', 'shipping', 1.7, 4.1),
        call('4', 'triage', 7, 8.8, { status: 'failed', error: 'Upstream timed out' }),
      ]}
    />
  ),
}

export const Running: StoryObj = {
  render: () => (
    <Flow
      label="2 calls across 2 agents, one still running."
      endS={null}
      nowS={6}
      calls={[call('1', 'research', 0.3, 2.5), call('2', 'writer', 2.7, null)]}
    />
  ),
}

export const PausedForAHuman: StoryObj = {
  render: () => (
    <Flow
      label="2 calls across 2 agents, waiting on a person."
      endS={null}
      nowS={1_800}
      routed={false}
      calls={[
        call('1', 'refunds', 0.2, 3.1),
        call('2', 'approver', 3.3, null, { status: 'awaiting_human' }),
      ]}
    />
  ),
}
