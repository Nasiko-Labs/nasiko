/** Every user-facing flows string (plans/feat-flows.md). */
import type { Call } from './calls'
import type { DisplayStatus } from './status'

export const copy = {
  page: {
    loading: 'Loading the flow',
    what: 'this flow',
    notFound: 'Flow not found',
    notFoundHint:
      'There is no flow with this id that you can see. Flows are visible only to the user who ran them, and some traces (an agent called outside a request) never become flows.',
    stale: 'Couldn’t refresh this flow; showing what was last read.',
    noAccess: 'You no longer have access to this flow',
    noAccessHint: 'OpenRuntime stopped showing it to you, so this page stopped updating it.',
    copyId: 'Copy the flow id',
    rootAgent: (name: string) => `Started by ${name}`,
    orchestrator: 'The Orchestrator',
  },
  callTo: (agent: string) => `Call to ${agent}`,
  actions: { openTrace: 'Open trace', openChat: 'Open chat' },
  status: {
    running: 'Running',
    paused: 'Paused',
    finishing: 'Finishing',
    completed: 'Completed',
    failed: 'Failed',
    unknown: 'Unknown status',
  } satisfies Record<DisplayStatus, string>,
  stepStatus: {
    pending: 'Pending',
    running: 'Running',
    completed: 'Completed',
    failed: 'Failed',
    awaiting_human: 'Waiting on a human',
    resumed: 'Resumed',
  } as Record<string, string>,
  /** A4 / O5: the server's `completed` came early. */
  markedEarly: {
    running: 'Still running: the server marked this flow completed early.',
    paused: 'Still waiting on a human: the server marked this flow completed early.',
    finishing:
      'Finishing: agent calls were still running when the server marked this flow completed.',
    completed:
      'The server marked this flow completed before its last agent calls ended; the duration is from the trace.',
  } satisfies Partial<Record<DisplayStatus, string>>,
  kpi: {
    status: 'Status',
    duration: 'Duration',
    started: 'Started',
    steps: 'Steps',
    agents: 'Agents',
    waiting: 'Waiting on human',
  },
  panel: {
    title: 'Timeline',
    recorded: 'Recorded steps',
    merged: 'Recorded steps + calls from traces',
    fromTraces: 'Timing from traces (agent-to-agent calls aren’t recorded as steps yet)',
    checking: 'Checking traces for agent-to-agent calls…',
    partial: 'Partial: traces unavailable',
    expired: 'Traces for this flow have expired',
    retry: 'Retry',
    wholeSeconds: 'Timing to the nearest second',
    critical: 'Critical path',
    criticalUnavailable: 'Critical path unavailable',
    criticalWhy: {
      timing: 'needs millisecond timing for every call',
      parents: 'needs to know which agent made each call',
      everything: 'every call is on it',
    },
    more: (n: number) => `${n} more ${n === 1 ? 'call' : 'calls'} in the trace`,
    orchestrator: 'Orchestrator',
    parallel: (n: number) => `${n} in parallel`,
    waiting: (d: string) => `⋯ ${d} waiting ⋯`,
    waitingOnYou: 'Waiting on you',
    showLanes: (n: number) => `Show ${n} more ${n === 1 ? 'agent' : 'agents'}`,
    hideLanes: 'Show fewer agents',
    overflow: (n: number) => `+${n}`,
    from: (caller: string) => `from ${caller}`,
    maybeSame: 'may be the same call as another',
    unknownAgent: 'Unknown agent',
    summary: (calls: number, agents: number) =>
      `Timeline of ${calls} agent ${calls === 1 ? 'call' : 'calls'} across ${agents} ${agents === 1 ? 'agent' : 'agents'}.`,
    tableFirst: 'This flow has too many calls to draw; the steps below list them all.',
  },
  steps: {
    title: 'Steps',
    subtitle: 'Every agent call, in the order it started.',
    parallel: (n: number) => `Parallel ×${n}`,
    input: 'Input',
    output: 'Output',
    reason: 'What went wrong',
    timing: 'Timing',
    startedAfter: (d: string) => `Started ${d} in`,
    took: (d: string) => `took ${d}`,
    stillRunning: 'still running',
    fromTrace: 'from trace',
    maybeSame:
      'May be the same call as another one to this agent (two calls started less than a second apart).',
    openAgent: 'Open agent',
    openInTrace: 'Open this call in the trace',
    noInput: 'No input recorded.',
  },
  callCard: {
    title: 'Call',
    direct: (agent: string) => `A direct call to ${agent}; no other agents were recorded.`,
    input: 'Request',
  },
  announce: (status: string) => `Flow ${status.toLowerCase()}.`,
  view: { label: 'View', timeline: 'Timeline', table: 'Table' },
  table: {
    label: 'Calls and their timing',
    agent: 'Agent',
    start: 'Start',
    end: 'End',
    duration: 'Duration',
    status: 'Status',
    parallel: 'Parallel group',
    critical: 'Critical path',
    source: 'Source',
    group: (n: number) => `Group ${n}`,
    yes: 'Yes',
    recorded: 'recorded',
  },
  answer: { request: 'Answer the request', waiting: 'Open waiting requests' },
  openFlow: 'Open flow',
  openLatestFlow: 'Open latest flow',
  back: 'Flows',
  backLabel: 'Back to Flows',
  list: {
    title: 'Flows',
    description:
      'Every request to your agents, newest first: routed chats, direct calls and workflow steps.',
    search: 'Search by question or agent',
    kind: 'Kind',
    statusLabel: 'Status',
    kinds: { all: 'All', orchestrated: 'Orchestrated', direct: 'Direct', workflow: 'Workflow' },
    statuses: {
      all: 'All',
      running: 'Running',
      paused: 'Paused',
      completed: 'Completed',
      failed: 'Failed',
    },
    sample: (n: number) => `From your latest ${n} ${n === 1 ? 'flow' : 'flows'}`,
    capped: (n: number) => `Stopped at ${n} flows; narrow the window or search to see older ones.`,
    outOfReach: (n: number) => `This window is older than your latest ${n} flows`,
    outOfReachBody: 'OpenRuntime can’t list further back yet. Pick a more recent window.',
    readFailed: 'Some older flows couldn’t be read.',
    stale: 'Couldn’t refresh the list; showing what was last read.',
    retry: 'Retry',
    requests: 'Requests',
    requestsSummary: (n: number, failed: number) =>
      `${n} ${n === 1 ? 'request' : 'requests'} in this window, ${failed} failed.`,
    duration: 'Duration',
    durationSub: 'Finished flows only.',
    p99Hidden: (n: number) => `p99 shows from ${n} finished flows.`,
    noDurations: 'No finished flows in this window yet.',
    table: 'Table',
    chart: 'Chart',
    durationView: 'Duration view',
    percentile: 'Percentile',
    value: 'Duration',
    count: (n: number) => `${n} finished ${n === 1 ? 'flow' : 'flows'}`,
    cols: {
      question: 'Question',
      agent: 'Agent',
      status: 'Status',
      duration: 'Duration',
      started: 'Started',
    },
    tableLabel: 'Flows',
    kindIcon: { orchestrated: 'Orchestrated', direct: 'Direct call', workflow: 'Workflow step' },
    firstRun: 'No flows yet',
    firstRunBody: 'Every chat with an agent or the Orchestrator becomes a flow.',
    openChat: 'Open Chat',
    filteredEmpty: (status: string | null, q: string | null) =>
      `No ${status ? `${status.toLowerCase()} ` : ''}flows${q ? ` match “${q}”` : ''} in this window.`,
    clear: 'Clear filters',
    absent: 'Flows need a newer OpenRuntime server.',
    absentBody: 'This server has no /api/flows route.',
    what: 'your flows',
    loading: 'Loading flows',
  },
}

/** A map entry for a key the server sent: own keys only, so a status like "constructor" never reads Object.prototype. */
export const own = <V>(map: Readonly<Record<string, V>>, key: string): V | undefined =>
  Object.hasOwn(map, key) ? map[key] : undefined

/** A recorded step's status in words; a word this page doesn't know shows as sent. */
export const stepStatusLabel = (status: string) => own(copy.stepStatus, status) ?? status

/**
 * A failed call's reason in plain words (F15): never the server's raw text. Only the error is read, never the agent's
 * output (an agent shouldn't steer the reason it is shown failing with), and only whole words count ("generate" is
 * not a rate limit).
 */
export function reasonFor(call: Pick<Call, 'error'>): string {
  const raw = (call.error ?? '').toLowerCase()
  if (/\b(timed? ?out|timeout|deadline)\b/.test(raw)) return 'the agent didn’t answer in time'
  if (/\b(unreachable|connection refused|connection reset|dns)\b/.test(raw))
    return 'the agent couldn’t be reached'
  if (/\b(denied|forbidden|403|not allowed)\b/.test(raw))
    return 'the agent wasn’t allowed to do this'
  if (/\b(rate limit(ed)?|429|quota|too many requests)\b/.test(raw))
    return 'the agent hit a rate or usage limit'
  if (/\b(cancell?ed|stopped|dismissed)\b/.test(raw)) return 'the call was stopped'
  return 'the agent reported an error'
}
