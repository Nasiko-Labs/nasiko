/**
 * Every user-facing string for Sessions and Trace (plan: DX "copy.ts"). Tests assert
 * against these, so wording changes happen here once. Degraded states always say what
 * to do next, not just what went wrong.
 */
import type { Status } from './sessions'
import {
  FLEET_PAGE_SIZE,
  SCAN_MAX_PAGES,
  SCAN_PAGE_SIZE,
  SCAN_PAGES,
  STATUS_TRACES_PER_SESSION,
} from './tuning'

export const STATUS_LABEL: Record<Status, string> = {
  failed: '✕ failed',
  ok: '✓ ok',
  unchecked: '— unchecked',
  unknown: '? unknown',
  checking: '… checking',
}

export const copy = {
  // Sessions → Open chat (Chat v1c §5.10; never "session" for a chat, ND-12).
  openChat: 'Open chat',
  probeFailed: "Couldn't check chat availability",
  probeFailedCause: 'OpenRuntime did not answer whether this is one of your chats.',
  retryProbe: 'Retry',
  copyProbeDetails: 'Copy details',
  failingChip: (n: number, checked: number) => `${n} failing · ${checked} checked`,
  slowChip: (n: number) => `${n} slow`,
  costlyChip: (n: number) => `${n} costly`,
  statusCaption: (checked: number, total: number) => `Status checked for ${checked} of ${total}`,
  laneScope: (day?: string) => (day ? `p95 within ${day}` : 'p95 over loaded sessions'),
  notEnoughToRank: 'Not enough sessions to rank',
  nothingNeedsAttention: 'Nothing needs attention',
  failingTooltip: `Failing = an unrecovered span error in one of the session's ${STATUS_TRACES_PER_SESSION} largest traces (from span status; a retry that worked is not a failure).`,

  viewingDay: (day: string) => `Viewing ${day}`,
  clearDay: 'Clear day',
  scanning: (page: number, of: number) => `Scanning sessions… (page ${page} of ${of})`,
  scanCapped: (day: string, scanned: number) =>
    `Scanned the ${scanned} most recent sessions since ${day}; a server day filter is recommended.`,
  scanMissedDay: (day: string) => `No sessions from ${day} were in the scanned range.`,
  scanMore: `Scan ${SCAN_MAX_PAGES - SCAN_PAGES} more pages`,
  scanPartial: (scanned: number, page: number, maxPages: number) =>
    `Scanned ${scanned} of up to ${maxPages * SCAN_PAGE_SIZE}; page ${page} failed.`,
  endOfRange: 'End of range',
  fleetSeekCapped: (pages: number) =>
    `The newest ${pages * FLEET_PAGE_SIZE} sessions are all after this window. Load more to keep going back.`,
  pastWindow: 'Past window · Live off',
  livePollFailed: 'Live update failed; showing the sessions loaded so far.',
  loadMore: 'Load more',

  divergence: (sessions: string, total: string, own: boolean) =>
    `${own ? 'Your chat sessions only' : 'Chat sessions only'} · ${sessions} of ${total} this day`,
  mixedSources: (mocked: 'Sessions' | 'TokenOps') => `${mocked} from mock data`,
  unappliedFilter: (what: string, value: string) =>
    `${what} filter (${value}) isn't applied here: sessions don't carry ${what.toLowerCase()}.`,
  unknownAgentFilter: (agent: string) => `No sessions for agent "${agent}" in this window.`,
  unknownAgent: '(unknown agent)',
  newSessions: (n: number) => `${n} new session${n === 1 ? '' : 's'}`,

  emptyTitle: 'No sessions in this window',
  emptyBody: 'Try a wider time window.',
  noMatchTitle: 'No sessions match these filters',
  noMatchBody: 'Clear the agent, lane and status filters to see every session in this view.',
  clearFilters: 'Clear filters',
  sparseLive:
    'Sessions come from chat traffic: run `nasiko chat <agent>` against a deployed agent, or use mock data with `npm run dev`.',
  noTraceData:
    'No trace data found for these sessions: the agents may not export traces yet, or nasiko-server has no trace store configured.',
  noTraceDataFix:
    'Set TEMPO_URL and LOKI_URL on nasiko-server (README › Live data), then reload. Each session still opens on its own page.',
  noTraceDataLongWindow:
    'Trace details are missing for this window, most likely because the server searches Tempo across all of it and Tempo refuses searches of 7 days or more.',
  noTraceDataLongWindowFix:
    'Open a session to see its trace, or pick 7d. If 7d is empty too, the trace store may be down (README › Live data).',
  notConfigured: "Observability isn't configured on this server.",
  notConfiguredFix:
    'Set TEMPO_URL and LOKI_URL on nasiko-server (README › Live data) and restart it.',
  traceStoreError: 'The trace store returned an error.',
  notFound: 'Not found, or not visible to you.',
  noAccess: "You don't have access to this.",
  noAccessFix: 'Ask an admin or the owner for access.',
  notFoundSession:
    "This session isn't available: it doesn't exist, or it belongs to another user (non-superusers see only their own sessions).",
  serverDown: (apiUrl: string) =>
    `Is nasiko-server running at ${apiUrl}? Run \`just run-stack\` in nasiko-cloud-rs.`,

  traceNotFound: 'Trace not found, or not visible to you.',
  noTraces: 'No traces recorded for this session',
  noTracesBody:
    'The agent may not export traces, or they are still on their way: check back in a minute.',
  stillCollecting: 'Still collecting spans…',
  captureOff:
    'Content capture is off for this agent (OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT).',
  unknownSpan: "That span isn't in this trace; showing the default span instead.",
  linkCopied: 'Link copied',
  copyFailed: 'Copy failed: select the address bar and copy it instead.',
  logsEmpty: 'No log lines yet',
  logsUnavailable: 'Logs unavailable for this agent (deleted, or not visible to you).',
  streamPaused: 'Stream paused',
  reconnect: 'Reconnect',
  pauseLive: 'Pause live updates',
  filterSpans: 'Filter spans',
  spanCount: (shown: number, total: number, filtered: boolean) =>
    filtered ? `${shown} of ${total} spans` : `${total} spans`,
  noSpansMatch: 'No spans match this filter.',
  rollup: (c: { spans: number; llm: number; tool: number; agent: number }) =>
    [
      `${c.spans} spans`,
      c.llm ? `${c.llm} LLM call${c.llm === 1 ? '' : 's'}` : '',
      c.tool ? `${c.tool} tool call${c.tool === 1 ? '' : 's'}` : '',
      c.agent ? `${c.agent} agent call${c.agent === 1 ? '' : 's'}` : '',
    ]
      .filter(Boolean)
      .join(' · '),
  copied: 'copied',
  prevSpan: 'Previous span',
  nextSpan: 'Next span',
  copySpanId: 'Copy span ID',
  filteredOut: 'filtered out',
  resumeLive: 'Resume live updates',
} as const
