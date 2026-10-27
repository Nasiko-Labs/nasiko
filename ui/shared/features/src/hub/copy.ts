/**
 * Every user-facing string on the agent hub (plans/feat-agent-centric.md PR1a; design and DX amendments give the exact
 * wording). Two words with fixed meanings (DX "Errors"): "not reported" = the server lacks the field; "not recorded"
 * = no record exists.
 */

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`
const sessionsN = (n: number) => plural(n, 'session', 'sessions')

export const copy = {
  section: 'Activity over time',
  notReported: 'not reported',

  bar: {
    compare: 'Compare',
    compareHint: 'Draw the previous window under this one',
    compare24h:
      'Hourly buckets have no previous window to draw: the lane headers show the totals change only.',
    table: 'Table',
    chart: 'Chart',
  },

  lanes: {
    spend: 'Spend',
    requests: 'Requests',
    requestsHint: 'Traced requests (finops operations), not sessions or flows',
    p95: 'p95 latency',
    peakP95: 'peak daily p95',
    peakHourlyP95: 'peak hourly p95',
    saved: 'Tokens saved',
    savedBasis: (basis: string) => `Basis: ${basis}`,
    savedTooltip: 'Fewer tokens, not necessarily better answers.',
    noDailySavings: 'Daily savings not reported yet',
    savedNotReported: 'Tokens saved: not reported',
    zeroRequests: '0 requests',
    truncated: (n: number) =>
      `Showing the first ${n} buckets of this window; pick a shorter window to see the rest.`,
    notDeployedYet: 'Not deployed yet',
    previousWindow: 'Previous window',
    estCost: 'Est. cost (API list price)',
    requestsCount: (n: number) => plural(n, 'request', 'requests'),
  },

  states: {
    notVisible: "You can't see this agent's spend. Ask its owner for access.",
    notVisibleOwner: (owner: string) => `You can't see this agent's spend. Its owner is ${owner}.`,
    savingsError: "Savings couldn't load.",
    retry: 'Retry',
    deleted: 'This agent was deleted, so it has no activity to show.',
    notVisibleSameName:
      "Spend can't be shown: another user's agent has the same name, and the server checks them together.",
    previousError: "The previous window couldn't load, so Compare draws nothing.",
  },

  budgetSection: 'this agent’s budget',
  rail: {
    label: 'Changes',
    showing: 'Showing recorded deploys and the latest crash',
    showingOss: 'Showing recorded builds and versions',
    harness: 'No recorded changes for this harness yet',
    otherLater:
      'Other changes (routing, optimization, tools, restarts) appear when the server records them.',
    noDeploys: 'No deploys in this window.',
    capped: (n: number) => `Latest ${n} build records checked`,
    deploysFailed: "Deploys couldn't load",
    changesFailed: "Changes couldn't load",
    eventsTruncated: 'More changes happened in this window than were read.',
    preview: 'Preview: needs server support',
    previewHint:
      'Restarts, routing, optimization, tool and budget changes come from a proposed event log (mock data).',
    flagLabel: (count: number, first: string, when: string) =>
      count === 1 ? `${first}, ${when}` : `${count} changes, ${when}`,
    approximate: 'approximate time (the build record’s last update)',
  },

  marker: {
    buildDone: 'Build completed',
    buildFailed: 'Build failed',
    versionRecorded: 'Version recorded',
    crashed: 'Crashed',
    openBuild: 'Open build',
    versions: 'Versions',
    by: (actor: string) => `by ${actor}`,
  },

  bucket: {
    title: (name: string) => `Selected: ${name}`,
    close: 'Clear selection',
    sessionsDay: 'Sessions on this day',
    sessionsOn: (date: string) => `Sessions on ${date}`,
    sessionsShared: (name: string, n: number) =>
      `Sessions named ${name} on this day (shared by ${n} agents)`,
    tokenopsDay: (date: string) => `TokenOps for ${date}`,
    olderNote: (days: number) => `Sessions search covers the last ${days} days.`,
    noMarkers: 'No recorded changes in this bucket.',
    announce: (name: string) => `${name} selected`,
    announceCleared: 'Selection cleared',
  },

  table: {
    label: 'Activity by bucket',
    bucket: 'Bucket',
    markers: 'Changes',
    action: 'Action',
    select: 'Select',
  },

  needs: {
    title: 'Needs you',
    clear: 'No issues found in checked data',
    checked: (parts: string[]) => `Checked ${parts.join(', ')}.`,
    partRequests: 'pending requests',
    partStatus: 'its health (rated on the last 7 days)',
    partBudget: 'its budget',
    budgetBadge: 'Budget',
    reviewBudget: 'Review the budget',
    // An empty sample starts at the agent's creation or 7 days back, so it means this agent had none in 7 days (QA).
    partSessions: (checked: number, found: number, scanned: number) =>
      scanned === 0
        ? "this agent's sessions (none in the last 7 days)"
        : found === 0
          ? `the latest ${sessionsN(scanned)} you can see (none of them this agent's)`
          : `${checked} of this agent's ${found} sessions in the latest ${scanned} you can see`,
    unproven: (n: number) =>
      `${n} pending ${n === 1 ? 'request' : 'requests'} can't be tied to your chats yet, so requests aren't fully checked.`,
    shared: '(another visible agent has the same name: some may be its sessions)',
    requests: (n: number) => `${n} ${n === 1 ? 'request is' : 'requests are'} waiting for you`,
    answer: 'Answer',
    statusAction: 'Needs action',
    statusWatch: 'Watch',
    ratedOn: 'rated on the last 7 days',
    sessions: (failed: number, checked: number) =>
      `${failed} of the ${checked} sessions checked failed`,
    openFailed: 'Open failed sessions',
    failed: {
      requests: "Pending requests couldn't load",
      status: "Its health couldn't be rated",
      statusIncomplete: "Its health couldn't be fully rated (a cost input is missing)",
      sessions: "Its sessions couldn't be checked",
      budget: "Its budget couldn't be checked",
    },
  },

  runs: {
    sessions: 'Recent sessions',
    sessionsWindow: 'last 7 days',
    sessionsNone: (scanned: number) =>
      scanned === 0
        ? 'None in the last 7 days.'
        : `None in the latest ${sessionsN(scanned)} checked.`,
    sessionsAll: (name: string) => `Sessions for ${name}`,
    harnessWindow: 'your recorded sessions',
    /** Recorded harness sessions share one title on the server ("Coding session"): rows are told apart by time. */
    harnessRow: (when: string, messages: number | null | undefined) =>
      messages == null ? when : `${when} · ${plural(messages, 'message', 'messages')}`,
    harnessNone: 'No recorded sessions with this harness yet.',
    sessionsShared: 'Another visible agent has the same name: these may be its sessions.',
    sessionsError: "Sessions couldn't load",
    flows: 'Flows started by this agent',
    flowsNone: 'No flows started by this agent in this window.',
    flowsNoneCapped: (n: number) => `None in the latest ${n} flows across the fleet.`,
    flowsAll: 'Open Flows',
    flowsError: "Flows couldn't load",
    flowsPartial: "Some flows couldn't load: this list may be missing older ones.",
    failed: 'failed',
  },

  cards: {
    title: 'Layers',
    routing: 'Routing',
    routingNone: 'Not routed: uses the default model',
    routingSetUp: 'Set up routing',
    routingOpen: 'Open LLM router',
    routingPinned: 'pinned',
    tools: 'Tools',
    toolsConnected: (n: number) => `${n} connected`,
    toolsBlocked: (n: number, capped: boolean) =>
      `${n} ${n === 1 ? 'tool' : 'tools'} set to Block${capped ? ' (first connectors checked)' : ''}`,
    toolsNone: 'No tools connected',
    toolsSetUp: 'Connect tools',
    toolsOpen: 'MCP tab',
    optimization: 'Optimization',
    compressOn: 'Token optimization on',
    compressOff: 'Token optimization off',
    historyBy: (name: string) => `History: ${name}`,
    savedIn: (tokens: string, basis: string) => `${tokens} tokens saved in this window (${basis})`,
    optimizationOpen: 'Open Optimization',
  },

  ledger: {
    budgetSection: 'this session’s budget',
    trace: 'Trace',
    previewColumns:
      'Context, Memory, Controls applied and Budget use come from proposed server records.',
    title: 'Session ledger',
    label: 'Turns in this session',
    turn: 'Turn',
    time: 'Time (UTC)',
    prompt: 'Prompt',
    tools: 'Tools',
    tokens: 'Tokens in / out',
    cost: 'Cost',
    estCost: 'Est. cost (API list price)',
    running: 'Running total',
    context: 'Context',
    memory: 'Memory',
    controls: 'Controls applied',
    budgetUse: 'Budget use',
    noTools: 'No tool calls',
    turnCut: 'Started before the loaded messages',
    turnNotRecorded: 'Turn not recorded',
    unplacedNote: (n: number) =>
      `${plural(n, 'call', 'calls')} the harness couldn't tie to a turn.`,
    contextLine: (kept: string, pool: string) => `${kept} of ${pool} tokens`,
    memoryLine: (tokens: string, items: number | null) =>
      items === null ? `${tokens} tokens` : `${tokens} tokens · ${plural(items, 'item', 'items')}`,
    outcome: {
      blocked: 'blocked',
      asked: 'asked first',
      trimmed: 'trimmed',
      warned: 'warned',
      stopped: 'stopped',
    } as Record<string, string>,
    controlLine: (rule: string, outcome: string, tool?: string) =>
      tool ? `${rule}: ${tool} ${outcome}` : `${rule}: ${outcome}`,
    trimmedSaved: (tokens: string) => `${tokens} tokens removed`,
    totals: {
      label: 'Session totals',
      turns: (n: number) => plural(n, 'turn', 'turns'),
      /** Only some turns reported the value: the sum is a floor over those. */
      partly: (value: string, reported: number, of: number) =>
        `≥ ${value} (${reported} of ${of} turns reported)`,
      calls: (n: number, failed: number) =>
        `${plural(n, 'tool call', 'tool calls')}${failed ? ` · ${failed} failed` : ''}`,
      tokens: (tin: string, tout: string) => `${tin} in · ${tout} out`,
      subtotal: 'Loaded subtotal: older messages are not loaded yet',
      complete: 'Whole session',
    },
    loadOlder: 'Load older turns',
    loadingOlder: 'Loading older turns…',
    olderError: "Older turns couldn't load",
    exportCsv: 'Export CSV',
    csvName: (id: string) => `ledger-${id.slice(0, 8)}.csv`,
    csvSubtotal: 'Exports the loaded turns only',
    loading: 'Loading the ledger',
    refreshError: "Couldn't refresh the ledger: it shows the turns as last loaded.",
    notOwner:
      "No recorded messages you can read for this session: only a chat's owner can read its messages, and a session without a recorded chat has none. Here is what the trace recorded.",
    notOwnerTraces: (traces: number, cost: string) =>
      `${plural(traces, 'trace', 'traces')} · ${cost}`,
    notOwnerNoTrace: 'No trace data is available for this session either.',
    empty: 'No messages were recorded for this session.',
    metadataOnly:
      'This session was recorded under the metadata-only policy: the harness kept counts, not messages or tool calls.',
    cachedLine: (tokens: string) => `incl. ${tokens} cached`,
    budgetSessionOnly: (pct: string) => `${pct} by this session (earlier use not recorded)`,
    budgets: {
      title: 'Budget use',
      session: "this session's budget",
      agent: (month: string) => `this agent's ${month} budget`,
      owner: (month: string) => `your ${month} budget`,
      used: (pct: string, name: string) => `This session used ${pct} of ${name}`,
      before: (pct: string) => `; ${pct} was used before it started.`,
      beforeNotRecorded: '; use before it started was not recorded.',
      period: '.',
      tightest: 'Tightest',
      none: 'No budget applies to this session.',
      error: "Budgets couldn't load",
    },
    csv: {
      headers: {
        turn: 'turn',
        time: 'time (UTC)',
        tools: 'tool names',
        tokensIn: 'tokens in',
        cacheRead: 'of which cache reads',
        cacheWrite: 'of which cache writes',
        tokensOut: 'tokens out',
        context: 'context tokens',
        memory: 'memory tokens',
        controls: 'controls applied',
        source: 'source',
        coverage: 'coverage',
      },
      notRecorded: 'not recorded',
      source: { recorded: 'recorded', preview: 'preview (mock data)' },
      coverage: (complete: boolean, turns: number) =>
        complete ? 'whole session' : `latest ${plural(turns, 'turn', 'turns')} loaded`,
    },
    controlsNow: {
      title: 'Controls set now',
      hint: 'Current settings, not what applied to these turns. Change them where they are set.',
      tools: 'Tool rules (MCP tab)',
      optimization: 'Token optimization (Optimization)',
      budgets: 'Budgets (LLM router)',
    },
  },

  keyboard: 'Arrow keys move between buckets; Enter selects one.',
  bucketButton: (name: string, spend: string, requests: string) => `${name}: ${spend}, ${requests}`,
}
