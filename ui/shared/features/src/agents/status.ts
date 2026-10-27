/**
 * The one status matrix (plan §6.1): raw `agents.status` → display status → allowed
 * lifecycle actions, badge tone and tooltip. Rows, the detail header, the Your agents tabs
 * and tests all read this table; nothing else maps statuses.
 */

export type DisplayStatus =
  'running' | 'deploying' | 'attention' | 'stopped' | 'not-deployed' | 'harness' | 'unknown'
export type LifecycleAction = 'restart' | 'stop' | 'start'
type BadgeTone = 'success' | 'info' | 'warning' | 'muted' | 'outline'

interface StatusInfo {
  label: string
  tone: BadgeTone
  actions: readonly LifecycleAction[]
  hint: string
}

export const STATUS: Record<DisplayStatus, StatusInfo> = {
  running: {
    label: 'Running',
    tone: 'success',
    actions: ['restart', 'stop'],
    hint: 'The runtime reports the container as running.',
  },
  deploying: {
    label: 'Deploying',
    tone: 'info',
    actions: ['stop'],
    hint: 'A deploy, restart or roll back is in progress.',
  },
  attention: {
    label: 'Needs attention',
    tone: 'warning',
    actions: ['restart', 'stop'],
    hint: 'The runtime reports the agent as crashed or failed.',
  },
  stopped: {
    label: 'Stopped',
    tone: 'muted',
    actions: ['start'],
    hint: 'The container is stopped.',
  },
  'not-deployed': {
    label: 'Not deployed',
    tone: 'muted',
    actions: [],
    hint: 'Registered but never deployed (status "registered").',
  },
  harness: {
    label: 'Coding harness',
    tone: 'outline',
    actions: [],
    hint: 'Reported by the nasiko CLI; it never runs as a container.',
  },
  unknown: {
    label: 'Unknown',
    tone: 'muted',
    actions: [],
    hint: 'The server reported a status this page does not know.',
  },
}

/** Order of the Your agents tabs after "All". */
export const TAB_STATUSES = ['running', 'deploying', 'attention', 'stopped'] as const

interface HarnessMarkers {
  tags?: string[] | null
  metadata?: unknown
  is_coding_agent?: boolean | null
}

/** A coding harness row: the list has only tags/metadata; the detail says so outright. */
export function isHarness(a: HarnessMarkers): boolean {
  if (a.is_coding_agent) return true
  if (a.tags?.includes('coding-agent')) return true
  const meta = a.metadata as { source?: unknown } | null | undefined
  return meta?.source === 'nasiko-cli-integration'
}

export function displayStatus(raw: string | null | undefined, harness: boolean): DisplayStatus {
  if (harness) return 'harness'
  switch (raw) {
    case 'running':
      return 'running'
    case 'deploying':
    case 'pending':
      return 'deploying'
    case 'crashed':
    case 'failed':
      return 'attention'
    case 'stopped':
      return 'stopped'
    case 'registered':
      return 'not-deployed'
    default:
      return 'unknown'
  }
}

/** Tooltip text: the hint plus the raw value when it isn't obvious from the label. */
export function statusTooltip(display: DisplayStatus, raw: string | null | undefined): string {
  const base = STATUS[display].hint
  return raw && display !== 'harness' ? `${base} Raw status: "${raw}".` : base
}

export function actionsFor(display: DisplayStatus): readonly LifecycleAction[] {
  return STATUS[display].actions
}
