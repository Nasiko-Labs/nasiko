/**
 * Every user-facing string on the Harnesses page (plan §6). Each notice says what
 * happened, why, and one thing to do about it (X4).
 */
export const copy = {
  title: 'Harnesses',
  subtitle: 'Coding harnesses connected to OpenRuntime',
  loading: 'Loading harness usage',
  registered: 'Registered',
  registeredTip:
    'A harness is registered when its agent row exists: `nasiko agents install` creates it, and so does `nasiko connect`, `use` or `auth login` for every harness found on the machine. Uninstalling keeps the row, so registered can overstate real use.',
  estCost: 'Est. cost',
  estCostTip:
    'Tokens × the model list price. Harness seats are usually billed per seat, so this is not an invoice.',
  csvCostHeader: 'Estimated cost (API list price, USD)',
  unpriced: 'unpriced',
  unpricedTip: 'Most of these turns used a model with no price, so no cost is shown.',
  unpricedCount: (n: number) => `${n.toLocaleString('en-US')} unpriced`,
  idle: 'idle',
  notConnected: 'Not connected',

  // Page status line (at most one). The org levels' own lines live in the EE layer.
  serverMissing: {
    problem:
      'Full harness usage is not available on this server (endpoint missing or not reachable).',
    // The proposed endpoint: docs/designs/openruntime-harness-recommendations.md (not shown to users).
    action: 'Showing your own usage from existing endpoints.',
  },
  previewBadge: 'preview (mock)',
  refreshFailed:
    "Couldn't refresh harness usage; the numbers below are from the last successful load.",

  // Footnotes.
  overlap: (n: number) =>
    `${n} ${n === 1 ? 'developer is' : 'developers are'} counted in more than one unit; the totals count everyone once.`,
  removedHarnesses: "Activity from removed harnesses isn't shown here.",
  sessionsOwnOnly: 'Only your own sessions open in Sessions.',
  lastSessions: (n: number) => `Last ${n} sessions`,
  noRecentSessions: 'No harness sessions yet.',
  showAllSessions: (n: number) => `Show all ${n}`,
  partialSort: (loaded: number, total: number) =>
    `Sorted within the ${loaded} loaded of ${total}; load more to include the rest.`,

  // Empty / not visible.
  noActivity: (windowLabel: string) => `No harness activity in the ${windowLabel.toLowerCase()}.`,
  widen: 'Try 30 days',
  noDevelopers: 'No developers in this unit.',
  noUnits: 'No units to show.',
  noDevelopersInScope: 'No developers in this scope.',
  notVisible: 'Not found or not visible.',
  notVisibleHint: "This usage doesn't exist, or it isn't shared with you.",
  noHarnesses: 'No harnesses connected to OpenRuntime yet.',
  deltaUnavailable: 'Δ unavailable',

  // Onboarding (viewer only).
  connectSteps: (server: string, harness: string) => [
    `nasiko connect ${server}`,
    'nasiko auth login',
    `nasiko agents install ${harness}`,
  ],
  connectHint:
    'The harness must be installed on this machine. `nasiko auth login` registers every harness it finds; `agents install` is the fallback.',
  askToConnect: (name: string, harness: string) => `Ask ${name} to connect ${harness}.`,
} as const
