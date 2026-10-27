/** Every user-facing Overview string (plans/feat-overview.md §13). */
import type { HitlKind } from '@nasiko/features/chat/types'
import { fmtShortDay } from '@nasiko/ui/lib/format'
import type { Rating, SpikeHit } from './health'
import type { Severity, SourceId } from './needs'

const pct = (v: number) => `${Math.round(v)}%`
const hello = (hour: number, name: string) =>
  `Good ${hour < 12 ? 'morning' : hour < 18 ? 'afternoon' : 'evening'}, ${name}.`

export const copy = {
  title: 'Home',
  loading: 'Loading',
  loadingPage: 'Loading Home',
  meFailed: "Couldn't reach OpenRuntime to load your account.",
  checkStatus: 'Check server status',
  retry: 'Retry',
  couldntLoad: (what: string) => `Couldn't load ${what}.`,
  couldntCheck: (what: string) => `Couldn't check ${what}`,
  summary: 'Summary',
  month: {
    // Fleet, never "your": the calendar is fleet-wide (eng review R2).
    label: 'Fleet spend this month',
    ofLast: (mtd: string, last: string) => `${mtd} of last month's ${last}`,
    noLast: (mtd: string) => `${mtd} · no spend last month`,
    over: 'over forecast',
  },
  headline: {
    checking: 'Checking your fleet…',
  },
  greeting: (hour: number, name: string, days: number) =>
    `${hello(hour, name)} Here is your workspace for the last ${days} days.`,
  /** The header's view switch: the cards, or the estate's canvas in either of its views. */
  views: {
    label: 'View',
    overview: 'Overview',
    village: 'Village',
    map: 'Map',
    loading: (view: 'village' | 'map') =>
      view === 'map' ? 'Loading the map' : 'Loading the village',
  },
  range: {
    label: 'Range',
    item: (days: number) => `Last ${days} days`,
  },
  setup: {
    button: 'Setup guide',
    title: 'Set up OpenRuntime',
    intro:
      'Deploy an agent from the app, or use the CLI. Cost, health and sessions show up on Home once it runs.',
  },
  // Overview's token-optimisation band. One figure, one next step, then hand off to TokenOps.
  savings: {
    eyebrow: 'Token optimization',
    eyebrowOff: 'Token optimization — not switched on',
    savedSuffix: 'saved',
    neverSent: (tokens: string) => `${tokens} tokens never sent.`,
    couldSave: (agent: string, spend: string) =>
      `${agent} has it switched off and spent ${spend} this period — turning it on is the biggest win left.`,
    mostly: (what: string) => `Mostly from ${what}.`,
    allOn: 'Every agent that can use it already has it on.',
    detail: 'See the breakdown',
    // Money on the table, not "$0 saved": one is a reason to act, the other reads as a broken feature.
    idleSuffix: 'spent that could be trimmed',
    idleLine:
      'Your agents re-send tool output and chat history the model does not need. Turning optimization on for one agent shows what it would save.',
    idleCta: 'Choose an agent',
  },
  kpi: {
    spend: 'Spend',
    runs: 'Agent runs',
    perDay: (n: string) => `${n} per day`,
    perRun: (cost: string) => `${cost} per run`,
    tokens: 'Tokens',
    tokensInput: (pct: string) => `input ${pct}`,
    tokensOutput: (pct: string) => `output ${pct}`,
    running: (n: number, total: number) => `${n} of ${total} running`,
    agents: 'Agents running',
    of: (total: number) => `/ ${total}`,
    agentParts: {
      deploying: (n: number) => `${n} deploying`,
      attention: (n: number) => `${n} crashed or failed`,
      stopped: (n: number) => `${n} stopped`,
      'not-deployed': (n: number) => `${n} not deployed`,
    },
    allRunning: 'All running',
    noAgents: 'No agents yet',
    harnesses: 'Coding harnesses used',
    connected: 'connected',
    est: 'est.',
    connectedCount: (n: number) => `${n} connected`,
    devs: (n: number) => `${n} developer${n === 1 ? '' : 's'}`,
    yourUsage: 'your usage',
  },
  saved: {
    title: 'Saved tokens by agent',
    link: 'TokenOps',
    what: 'savings by agent',
    subtitle: (basis: string | null) =>
      basis ? `Largest first · basis: ${basis}` : 'Largest first',
    capped: 'Agents past the first 200 may have savings too',
    none: 'No savings reported in this range.',
    other: (n: number) => `Other (${n})`,
  },
  chats: {
    title: 'Recent chats',
    link: 'Chat',
    what: 'recent chats',
    none: 'No chats yet.',
    start: 'Start one',
  },
  needs: {
    title: 'Needs you',
    what: 'what needs you',
    waiting: 'Waiting for you',
    checked: (ago: string) => `Checked ${ago}`,
    /** The header's counts by severity, in severity order. */
    count: {
      action: (n: number) => `${n} need action`,
      watch: (n: number) => `${n} to watch`,
      waiting: (n: number) => `${n} waiting`,
    } satisfies Record<Severity, (n: number) => string>,
    nothing: 'Nothing needs you right now.',
    more: (n: number) => `${n} more below: scroll the list`,
    source: {
      requests: 'waiting requests',
      agents: 'agent health',
      budgets: 'budgets',
      sessions: 'recent sessions',
    } satisfies Record<SourceId, string>,
    request: (kind: HitlKind, n: number) => {
      const what =
        {
          tool_approval: 'Approve a tool call',
          input_required: 'Input needed',
          auth_required: 'Sign-in needed',
        }[kind] ?? 'Request waiting'
      return n > 1 ? `${what} (+${n - 1} more)` : what
    },
    untitledChat: 'Untitled chat',
    review: 'Review',
    reviewLabel: (chat: string) => `Review the request in ${chat}`,
    outside: (n: number) => `${n} request${n === 1 ? '' : 's'} outside Chat`,
    outsideDetail: 'From workflows or tools, not a chat',
    openChat: 'Open Chat',
    // Accessible names start with the visible text (WCAG 2.5.3).
    openWaiting: 'Open Chat: the requests waiting outside Chat',
    seeFailing: 'See sessions: the failing ones',
    openAgent: 'Open agent',
    openAgentLabel: (name: string) => `Open ${name}`,
    budgets: 'Budgets',
    budgetsLabel: (name: string) => `Budgets for ${name}`,
    // An agent budget's row opens the agent's own Budget section (B5).
    agentBudget: 'Budget',
    agentBudgetLabel: (name: string) => `Budget for ${name}`,
    anAgent: 'An agent',
    yourBudget: 'Your monthly',
    budget: (label: string, state: 'warning' | 'exceeded', crossed: number | null) =>
      state === 'exceeded'
        ? `${label} budget is over its limit`
        : `${label} budget is at ${crossed ?? 0}%`,
    // plans/feat-budget-burn.md F8: how much over and whether calls stop; when it reaches its limit ("runs out" only
    // where calls stop, design 1C).
    budgetStopped: (label: string) => `${label} budget ran out: calls are stopped`,
    budgetOverBy: (amount: string, until: string | null) =>
      until ? `Over by ${amount} · calls stopped until ${until}` : `Over by ${amount} this period`,
    budgetRunsOut: (label: string, day: string, stops: boolean) =>
      stops
        ? `${label} budget runs out around ${day}`
        : `${label} budget exceeds its limit around ${day}`,
    budgetProjected: 'Projected from the last 7 days',
    budgetDetailFor: (
      period: 'month' | 'quarter' | 'session',
      covers: 'routed_calls' | 'all_priced',
    ) =>
      `${period === 'session' ? 'This session' : `This ${period}`}, ${covers === 'all_priced' ? 'all priced spend' : 'routed calls'}`,
    aBudget: 'A budget',
    fleetBudget: 'The fleet',
    budgetStoppedUntil: (until: string | null) =>
      until ? `Calls stopped until ${until}` : 'Calls stopped',
    // An exact count: `session/list?status=failed` counts every match in the
    // window in SQL. It used to read "N of the M latest sessions checked
    // failed", because the status of each row was sampled client-side.
    sessions: (failed: number) =>
      failed === 1
        ? '1 session failed in the last 7 days'
        : `${failed} sessions failed in the last 7 days`,
    seeSessions: 'See sessions',
    unknownAgent: 'Unknown agent',
  },
  harnesses: {
    title: 'Coding harnesses',
    link: 'Coding harnesses',
    what: 'harness usage',
    ownOnly: 'Your own usage (this server has no org view).',
    notVisible: 'No harness usage is visible to you.',
    none: 'No coding harnesses connected.',
    noneYet: 'No coding harnesses yet',
    connect: 'Connect one',
    connected: (n: number) => `${n} coding harness${n === 1 ? '' : 'es'} connected`,
    lineCost: (cost: string, days: number) =>
      `${cost} est. cost (API list price), last ${days} days`,
    open: 'Open Coding harnesses',
    unpriced: 'unpriced',
    costNote: (days: number) => `Est. cost (API list price), last ${days} days`,
  },
  sessions: {
    title: 'Recent sessions',
    colStatus: 'Status',
    colSession: 'Session',
    colAgent: 'Agent',
    colCost: 'Cost',
    colWhen: 'When',
    link: 'Sessions',
    what: 'recent sessions',
    failed: 'Failed',
    ok: 'Succeeded',
    running: 'Running',
    unchecked: 'Not checked',
    none: 'No sessions in the last 7 days.',
    traceStore: 'Sessions need the trace store.',
    open: 'Open Sessions',
  },
  actions: {
    title: 'Quick actions',
    newChat: 'New chat',
    orchestrator: 'Ask the Orchestrator',
    copyDeploy: 'Copy the deploy command',
    adjustBudget: 'Adjust budget',
  },
  /** The first run's panel of what works before any agent (components/SetUpNow.tsx). */
  setUpNow: {
    title: 'Works before your first agent',
    orchestrator: 'Chat now. Once agents run, it routes each question to the right one.',
    model: 'Connect a model',
    models: (n: number) => `${n} model config${n === 1 ? '' : 's'} ready`,
    noModel: 'No model connected yet',
    tools: 'Connect an MCP server',
    serversOn: (n: number) => `${n} tool server${n === 1 ? '' : 's'} connected`,
    serversOff: (n: number) => `${n} available, none connected yet`,
    noServers: 'No MCP servers yet',
    noHarnesses: 'None connected yet',
    unknown: "Couldn't check. Open the page for details.",
  },
  firstRun: {
    greeting: hello,
    title: 'Get your first agent running',
    headline:
      'OpenRuntime hosts it, routes its model calls and records every session. Once it runs, this page shows what it costs, how healthy it is and what needs you.',
    cli: (n: number) => `Or use the CLI (${n} commands)`,
    cliTitle: 'Or use the CLI',
  },
  /** The first run's preview of the cards a deploy brings: what each will show, never a number. */
  preview: {
    title: 'After your first deploy',
    note: 'These fill in once an agent runs.',
    needs: 'Agents that need action, requests waiting for you and failed sessions, worst first.',
    spend:
      "Fleet spend for the range against the previous one, by agent, with this month's forecast.",
    health:
      'Every agent rated Healthy, Watch or Needs action on reliability, cost, activity and latency.',
  },
  spend: {
    title: 'Spend',
    link: 'TokenOps',
    what: "this month's spend",
    rangeWhat: 'spend by agent',
    noSpendYet: 'No spend yet this month',
    forecast: (low: string, high: string) => `Forecast ${low}–${high}`,
    forecastFrom: (day: number) => `Forecast from day ${day}`,
    chartSummary: (days: number, total: string, peakDay: string | null, peak: string) =>
      `Fleet spend per day, last ${days} days: ${total} in all${peakDay ? `, peaking on ${peakDay} at ${peak}` : ''}. The Table view lists every day.`,
    byAgent: (days: number) => `by agent · last ${days} days`,
    vsPrevious: (days: number) => `vs previous ${days} days`,
    chartMode: 'Chart type',
    bars: 'Stacked bars',
    lines: 'Lines',
    avg: (amount: string) => `avg ${amount}/day`,
    other: 'Other',
    otherNote: 'the rest of the fleet',
    showTable: 'Table',
    showChart: 'Chart',
    colDay: 'Day',
    colTotal: 'Total',
    seeSessions: 'See sessions',
    seeDay: 'See that day in TokenOps',
    drivers: (days: number) => `Spend by agent, last ${days} days`,
    driversWhat: 'the top drivers',
    colAgent: 'Agent',
    colShare: 'Share of spend',
    colSpend: 'Spend',
    colAvg: 'Avg / day',
    colChange: 'Change',
    colShow: 'In chart',
    otherAgents: (n: number) => `Other (${n} agent${n === 1 ? '' : 's'})`,
    show: (name: string) => `Show ${name} in the chart`,
    noDrivers: (days: number) => `No spend on agents you can access in the last ${days} days.`,
    // Eng review R2: TokenOps' scope wording (TokenopsPage.tsx), shortened.
    scopeNote: 'The month total is fleet-wide; drivers only include agents you can access.',
  },
  budget: {
    title: 'Budgets',
    link: 'Budgets',
    what: 'budgets',
    of: (limit: string) => `of ${limit}`,
    used: (pct: string) => `${pct} used`,
    daysLeft: (n: number) => (n === 0 ? 'resets today' : `${n} day${n === 1 ? '' : 's'} left`),
    forecast: 'Forecast at month end',
    tooEarly: 'Too early to say',
    noSpend: 'No spend yet',
    overLimit: 'over the limit',
    alerts: 'Alerts at',
    atLimit: 'At 100%',
    alertOnly: 'Alert only',
    stopCalls: 'Stop calls',
    agents: 'Agent budgets',
    state: (s: 'ok' | 'warning' | 'exceeded', stopped: boolean) =>
      stopped ? 'Stopped' : s === 'exceeded' ? 'Over' : s === 'warning' ? 'Warning' : 'On track',
    none: 'No budgets set.',
    set: 'Set a budget',
    underBudget: (amount: string) => `Forecast stays ${amount} under budget`,
  },
  health: {
    title: 'Fleet health',
    link: 'Agents',
    what: 'the agent list',
    costWhat: 'cost data (cost ratings read Unknown)',
    noneDeployed: 'No agent is deployed yet, so there is nothing to rate.',
    noWatch: 'No agents to watch.',
    colAgent: 'Agent',
    colRating: 'Rating',
    colP95: 'p95, 7 days',
    /** Starts with the visible text, so voice control can say it (WCAG 2.5.3); names capped at 3 (/ship review). */
    countLink: (visible: string, names: readonly string[]) => {
      if (!names.length) return `${visible} agents`
      const shown = names.slice(0, 3).join(', ')
      return `${visible}: ${shown}${names.length > 3 ? ` and ${names.length - 3} more` : ''}`
    },
  },
  rating: {
    healthy: 'Healthy',
    watch: 'Watch',
    action: 'Needs action',
    unknown: 'Unknown',
  } satisfies Record<Rating, string>,
  /** Plural count labels for the Fleet health header. */
  ratingCount: {
    healthy: (n: number) => `${n} healthy`,
    watch: (n: number) => `${n} watch`,
    action: (n: number) => `${n} need${n === 1 ? 's' : ''} action`,
    unknown: (n: number) => `${n} unknown`,
  } satisfies Record<Rating, (n: number) => string>,
  reason: {
    status: (raw: string) => (raw === 'failed' ? 'deployment failed' : 'crashed'),
    stuckDeploying: 'stuck deploying',
    budgetStopped: 'budget stopped: calls are refused',
    overBudget: 'over budget this month',
    spike: (s: SpikeHit) => `drove a ${s.factor.toFixed(1)}× spend spike on ${fmtShortDay(s.date)}`,
    costUp: (rise: number) => `cost ↑ ${pct(rise)} vs last week`,
    costPerOp: (factor: number) => `cost per turn ${factor.toFixed(1)}× its 30-day average`,
    idle: 'idle 7 days while deployed',
    activityDown: (drop: number) => `activity ↓ ${pct(drop)} vs last week`,
    p95Up: (rise: number) => `p95 latency ↑ ${pct(rise)} vs last week`,
  },
} as const
