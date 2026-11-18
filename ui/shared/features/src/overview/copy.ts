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
  meFailed: "Couldn't reach Open Runtime to load your account.",
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
    title: 'Set up Open Runtime',
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
    toolCalls: (n: string) => `${n} tool calls`,
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
  /** A summary tile with nothing in the range says what would fill it, never a bare zero (quiet.ts). */
  quiet: {
    activityLabel: 'Spend · Tokens · Agent runs',
    activity: (days: number) => `No agent activity in the last ${days} days`,
    activityDetail:
      'Chat with an agent or run a workflow: what it spends, the tokens it uses and each run show up here.',
    activityBefore: (amount: string, days: number) =>
      `The ${days} days before spent ${amount}. Chat with an agent or run a workflow to pick it back up.`,
    startChat: 'Start a chat',
    spendUnpriced: 'No priced spend',
    spendUnpricedDetail:
      'The runs in this range used models with no list price, so their cost reads as zero.',
    spendNone: (days: number) => `No spend in the last ${days} days`,
    spendNoneDetail: 'Agents ran, but none of their calls had a cost.',
    tokens: 'No tokens reported',
    tokensDetail:
      'Agents ran, but sent no token counts. Agents report them in their OpenTelemetry GenAI spans.',
    runs: (days: number) => `No agent runs in the last ${days} days`,
    runsDetail: 'Chat with an agent or run a workflow to see runs and cost per run here.',
    seeTokenops: 'Open TokenOps',
    seeSessions: 'Open Sessions',
    harnessesNone: 'No coding harnesses connected',
    harnessesNoneDetail:
      "Connect Claude Code or Codex to see your team's coding-agent spend next to your agents'.",
    harnessesIdle: (days: number) => `No coding sessions in the last ${days} days`,
    harnessesIdleDetail: (n: number) =>
      `${n} connected. Cost and models show up after the next session.`,
    connect: 'Connect',
    openHarnesses: 'Open Coding harnesses',
  },
  /** A card's own quiet state (Card.tsx `CardQuiet`): what would fill it and the one step that does. On the Overview the
   *  cards below Needs you step out instead (`hideEmpty`, user 2026-11-06); Weave's catalog keeps these. */
  cardQuiet: {
    spend: (days: number) => `No spend in the last ${days} days`,
    spendDetail:
      "Spend by agent and by day shows up here after an agent's first priced model call.",
    spendBefore: (amount: string, days: number) =>
      `The ${days} days before spent ${amount}. Run something to pick it back up.`,
    spendAction: 'Run a workflow',
    sessionsDetail: 'Each conversation with an agent is a session, with its trace, cost and steps.',
    sessionsAction: 'Ask the Orchestrator',
    chatsDetail: 'Ask a question and the Orchestrator picks the agent to answer it.',
    health: 'No agent is running yet',
    healthDetail:
      "Start or deploy one and it's rated here on reliability, cost, activity and latency.",
    healthAction: 'Open Agents',
  },
  /** What the coding harnesses ran (components/HarnessActivity.tsx, 2026-11-06). */
  activity: {
    title: 'Coding harness activity',
    meta: (days: number) => `last ${days} days`,
    what: 'coding harness activity',
    models: 'Models',
    modelsNote: 'share of turns',
    otherModels: (n: number) => `Other (${n})`,
    namesOnly: (harness: string, model: string) => `${harness} mostly runs ${model}`,
    subagents: 'Subagents',
    subagentsNote: 'runs spawned by a session',
    tools: 'Tools',
    toolsNote: 'share of calls',
    calls: (n: string) => `${n} tool calls`,
    failed: (pct: string) => `${pct} failed`,
  },
  /** The coding harness preview (HarnessPreview.tsx): Home plays in place, Today to Month 1 on sample data, while no
   *  coding harness is connected (2026-11-06, canvas D). */
  harnessPreview: {
    watch: 'Watch the preview',
    bar: 'Preview with sample data',
    timeline: 'Preview timeline',
    jump: (stop: string) => `Jump to ${stop}`,
    sample: 'Sample data',
    play: 'Play the preview',
    pause: 'Pause the preview',
    exit: 'Exit preview',
    exitNow: 'Exit the preview now',
    day: (n: number) => `Day ${n}`,
    stops: [
      { label: 'Today', title: 'One command on a developer’s laptop' },
      { label: 'Day 1', title: 'Sessions start arriving' },
      { label: 'Week 1', title: 'Home fills in' },
      { label: 'Month 1', title: 'How your team codes' },
    ],
    terminal: [
      'Logged in as dev@yourcompany.com',
      'Found Claude Code and Codex, now reporting',
      'Importing past sessions in the background',
    ],
    back: (secs: number) => `Back to your Home in ${secs}s`,
    /** Said once, as the countdown starts. */
    backSoon: (secs: number) =>
      `Home comes back in ${secs} seconds. Choose Stay to keep the preview.`,
    stay: 'Stay',
    copyCommands: 'Copy commands',
    copied: 'Copied',
    connectYours: 'Connect yours',
  },
  /** "Did you know": one tip at a time, in the lead card when its savings band is empty (DidYouKnow.tsx). */
  tips: {
    title: 'Did you know?',
    next: 'Next tip',
    items: [
      {
        text: 'Ask the Orchestrator anything: it picks the agent best suited to each question by meaning, not keywords.',
        link: 'Ask the Orchestrator',
        to: '/chat',
      },
      {
        text: 'Workflows chain agents into one repeatable run, and pause for your approval when a step needs it.',
        link: 'Open Workflows',
        to: '/workflows',
      },
      {
        text: 'Every agent call is traced: Sessions shows each hop with its tokens, cost and latency.',
        link: 'Open Sessions',
        to: '/sessions',
      },
      {
        text: 'Flows draw a multi-agent request as one timeline, with call depth, fan-out and timeouts enforced on every hop.',
        link: 'Open Flows',
        to: '/flows',
      },
      {
        text: 'The LLM router gives every agent one key per provider, so you can swap models without a redeploy.',
        link: 'Open LLM router',
        to: '/router',
      },
      {
        text: 'Connect an MCP server once and share its tools with any agent, with Allow, Ask or Block per tool.',
        link: 'Open MCP servers',
        to: '/mcp',
      },
      {
        text: 'Token optimization trims the chat history and tool output agents re-send, with no code changes.',
        link: 'Open Optimization',
        to: '/optimization',
      },
    ],
  },
  saved: {
    title: 'Saved tokens by agent',
    link: 'TokenOps',
    what: 'savings by agent',
    subtitle: (basis: string | null) =>
      basis ? `Largest first · basis: ${basis}` : 'Largest first',
    capped: 'Agents past the first 200 may have savings too',
    other: (n: number) => `Other (${n})`,
  },
  chats: {
    title: 'Recent chats',
    link: 'Chat',
    what: 'recent chats',
    none: 'No chats yet.',
  },
  needs: {
    title: 'Needs your attention',
    what: 'what needs your attention',
    waiting: 'Waiting for you',
    checked: (ago: string) => `Checked ${ago}`,
    /** The header's counts by severity, in severity order. */
    count: {
      action: (n: number) => `${n} need action`,
      watch: (n: number) => `${n} to watch`,
      waiting: (n: number) => `${n} waiting`,
    } satisfies Record<Severity, (n: number) => string>,
    nothing: 'Nothing needs your attention right now.',
    /** All clear, with agents to watch or setup steps listed under it: "nothing" would contradict them. */
    nothingUrgent: 'Nothing urgent needs your attention.',
    /** Watch agents, promoted into an otherwise empty inbox (they live in Fleet health otherwise). */
    watch: (n: number) => `${n} agent${n === 1 ? '' : 's'} to watch`,
    /** Every one of them for the same reason ("idle 7 days while deployed"). */
    watchSame: (n: number, reason: string) => (n === 1 ? reason : `All ${reason}`),
    /** Mixed reasons, counted by what they're about. */
    watchBy: {
      reliability: (n: number) => `${n} less reliable`,
      cost: (n: number) => `${n} costing more`,
      activity: (n: number) => `${n} quieter`,
      latency: (n: number) => `${n} slower`,
    },
    reviewWatch: 'Review',
    reviewWatchLabel: 'Review: the agents to watch',
    /** What the all-clear vouches for: one line per source that answered with nothing. */
    clear: {
      requests: 'No requests waiting for you',
      agents: 'No agents need action',
      budgets: 'No budgets at risk',
      sessions: 'No failed sessions in the last 7 days',
    } satisfies Record<SourceId, string>,
    setupTitle: 'Setup',
    setupCount: (n: number) => `${n} to set up`,
    /** Setup items: a step while the thing isn't set up (`title`, `detail`, `action`); once it is, its name, how many
     *  exist and where to manage them (`doneTitle`, `done`, `manage`). The canvases' setup steps (`estate/setup.ts`)
     *  reuse them, and add the two only the estate can tell: a first call, and a coding harness. */
    setup: {
      chat: {
        title: 'Chat with an agent',
        detail: 'The first call brings in spend, models and traces.',
        action: 'Chat',
      },
      harness: {
        title: 'Connect a coding harness',
        detail: 'Claude Code, Codex and Cursor sessions, with what each costs.',
        action: 'Connect',
      },
      router: {
        title: 'Add an LLM router config',
        detail: 'One key per provider for every agent; swap models without a redeploy.',
        action: 'Add',
        doneTitle: 'LLM router',
        done: (n: string, one: boolean) => `${n} config${one ? '' : 's'} ready`,
        manage: 'Manage',
      },
      workflows: {
        title: 'Create a workflow',
        detail: 'Chain agents into one repeatable run that pauses for your approval.',
        action: 'Create',
        doneTitle: 'Workflows',
        done: (n: string, one: boolean) => `${n} workflow${one ? '' : 's'} deployed`,
        manage: 'Open',
      },
      mcp: {
        title: 'Connect an MCP server',
        detail: 'Give your agents tools, with Allow, Ask or Block per tool.',
        action: 'Connect',
        doneTitle: 'MCP servers',
        done: (n: string, one: boolean) => `${n} server${one ? '' : 's'} connected`,
        manage: 'Manage',
      },
    },
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
    lineCost: (cost: string, days: number) => `${cost} at API prices, last ${days} days`,
    open: 'Open Coding harnesses',
    unpriced: 'unpriced',
    // Not "cost" (user 2026-11-06): a subscription seat pays a flat fee, so this was never spent.
    costNote: (days: number) =>
      `Value at API prices, last ${days} days. Subscription seats are billed at a flat rate.`,
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
    timed_out: 'Timed out',
    unchecked: 'Not checked',
    none: 'No sessions in the last 7 days.',
    traceStore: 'Sessions need the trace store.',
    open: 'Open Sessions',
  },
  /** Quick actions (components/QuickActions.tsx): Weave's catalog only since the Overview dropped it (2026-11-06). */
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
    // An empty cell says what it asks for and why, never only that nothing is there (NAS-1448).
    model: 'Add your model key',
    models: (n: number) => `${n} model config${n === 1 ? '' : 's'} ready`,
    noModel:
      'Agents call their LLM with your own API key. Add one from OpenAI, Anthropic or another provider.',
    tools: 'Connect an MCP server',
    serversOn: (n: number) => `${n} tool server${n === 1 ? '' : 's'} connected`,
    serversOff: (n: number) => `${n} available. Connect one to give your agents its tools.`,
    noServers: 'Optional. Give your agents tools to call, such as GitHub or your own API.',
    noHarnesses:
      'Optional. Install the CLI and sign in to see your Claude Code, Codex and Cursor sessions here.',
    unknown: "Couldn't check. Open the page for details.",
  },
  firstRun: {
    greeting: hello,
    title: 'Get your first agent running',
    headline:
      'Open Runtime hosts it, routes its model calls and records every session. Once it runs, this page shows what it costs, how healthy it is and what needs you.',
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
  /**
   * The try-a-run card (components/TryAgentRun.tsx): the agent the viewer picked during setup, run once as it is, then
   * the same task with Token optimization on. Both runs are real; their tokens and cost come back from the run itself.
   */
  tryRun: {
    title: (agent: string) => `Your ${agent} is ready`,
    lead: 'You picked it during setup. Give it a real task right here, then run the same task with Token optimization on and compare what it costs.',
    requestLabel: 'Its first task: process this meeting transcript',
    /** A real transcript for the meeting notes processor (it declines anything under about 80 words). */
    request: `Weekly launch sync, Tuesday 10:00.
[00:00] Priya: Thanks for joining. Main topic is the Q3 launch. We planned October 6, but the payments API security review is still open.
[00:42] Sam: Right, I have two findings left on token expiry and the refund endpoint. I will finish the review by Friday.
[01:15] Priya: If the review closes Friday, can we launch on October 20 instead? That gives QA a full week.
[01:40] Maria: QA can do it, but I need the staging build by Monday the 13th, not later.
[02:05] Lee: Marketing can move the campaign to the 20th. Budget for the campaign stays at 40k, no change.
[02:30] Priya: Okay. Decision: we launch October 20. Sam closes the security review by Friday. Maria gets the staging build by Monday the 13th.
[02:58] Lee: I will update the campaign plan and send the new dates to the agency this week.
[03:20] Sam: One risk: if the refund endpoint fix needs a schema change, the review slips again. I'll flag it Wednesday.
[03:45] Priya: Good. Let's meet again next Tuesday to confirm.`,
    baseline: 'Run it',
    optimized: 'Run it optimized',
    starting: 'Starting your agent…',
    running: 'Running your agent…',
    completed: 'Your agent completed',
    /** `figures` is "1,560 tokens, $0.000448" or empty when the run reported no usage. */
    baselineDone: (agent: string, figures: string) =>
      figures ? `Your ${agent} completed the run: ${figures}.` : `Your ${agent} completed the run.`,
    optimizedDone: (agent: string, figures: string) =>
      figures
        ? `Your ${agent} completed the same run with Token optimization on: ${figures}.`
        : `Your ${agent} completed the same run with Token optimization on.`,
    figures: (tokens: string, cost: string) => `${tokens} tokens, ${cost}`,
    saved: (percent: number) => `${percent}% less than the run without it.`,
    seeRun: 'See what it did',
    updated: "Home's spend and tokens now include this run.",
    nextStep: 'Next: run the same task with Token optimization on.',
    /** In place of "Run it optimized" while the viewer has no LLM router config: optimization works through it. */
    needsRouter:
      'To run it optimized, set up the LLM router first: Token optimization works through it. Add a model and its key, then come back here and run the same task optimized.',
    setUpRouter: 'Set up the LLM router',
    failed: (reason: string) => `The run didn't finish: ${reason}`,
    retry: 'Try again',
    notDeployed: (agent: string) =>
      `${agent} isn't running in your workspace yet. Once it is, you can run it here.`,
    // While the Setup guide's deploy of it runs, and when it didn't start (NAS-1450).
    deployingTitle: (agent: string) => `Your ${agent} is deploying`,
    deploying: 'Deploying your agent…',
    deployingHint: 'Usually 1–5 min. You can run it here once it is up.',
    deployFailed: (agent: string, reason: string | null) =>
      reason ? `${agent} didn't start: ${reason}` : `${agent} was deployed, but didn't start.`,
    openAgents: 'Open Agents',
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
