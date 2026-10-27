/** The Map view's board: every string it shows. */
import { fmtMoney } from '@nasiko/ui/lib/format'

const usd0 = (n: number) => `$${Math.round(n).toLocaleString('en-US')}`
const usd = (n: number) => (Math.abs(n) >= 100 ? usd0(n) : fmtMoney(n))
const pct = (f: number) => `${Math.round(f * 100)}%`
const n = (k: number, one: string, many: string) => `${k} ${k === 1 ? one : many}`

export const boardCopy = {
  label: 'Org, Fleet Runtime and TokenOps',
  instructions:
    'Map of the workspace: who spends, what runs and where the tokens go. Drag to pan, Ctrl or ⌘ with the wheel to zoom, arrows to pan, plus and minus to zoom, 0 to fit. Tab to a part and press Enter to open it.',
  org: 'Org',
  fleet: 'Fleet Runtime',
  tokenops: 'TokenOps',
  agents: 'Agents',
  workflows: 'Workflows',
  tools: 'Connectors & tools',
  harness: 'Local harnesses',
  totalSpend: 'Total spend',
  routers: 'LLM Routers',
  providers: 'Providers & models',
  providersMeta: (p: number, m: number) =>
    `${n(p, 'provider', 'providers')} · ${n(m, 'model', 'models')}`,
  budget: 'Monthly budget',
  allowance: 'Your allowance',
  budgetOf: (limit: number) => `of ${usd0(limit)}`,
  budgetPace: (p: number) => `${pct(p)} pace`,
  budgetLabel: (spent: number, limit: number, p: number) =>
    `${usd0(spent)} of ${usd0(limit)} used, on pace for ${pct(p)} of it by month end`,
  projected: (x: number) => `On pace for ${usd0(x)}`,
  alertAt: (a: number) => `alert at ${pct(a)}`,
  ungoverned: 'Ungoverned',
  ungovernedTitle: 'Spend with no budget, plus the coding-harness spend a budget can only alert on',
  noBudget: 'No team has a budget yet.',
  noBudgetLine: 'No budget',
  alertOnly: 'Alert only',
  alertOnlyTitle: 'Coding-harness spend a budget counts but can only alert on',
  teamsOver: (k: number) => `${n(k, 'team', 'teams')} over pace`,
  teamsLoose: (k: number) => `${n(k, 'team', 'teams')} with no budget`,
  teamsOk: 'Every team is on pace',
  /** The bill: the range's spend added up by provider and by source, both to the same total. */
  bill: {
    period: (days: number) => `Last ${days} days`,
    byProvider: 'By provider',
    bySource: 'By source',
    agents: 'Agents',
    harnesses: 'Coding harnesses',
    otherProviders: 'Other providers',
    unattributed: 'No known model',
    unattributedTitle: 'Spend on a model no provider is known for',
    before: 'Before optimizations',
    saved: 'Saved',
    total: 'Total',
    perDay: 'Per day',
    line: (name: string, usd: string) => `${name}, ${usd} on the bill`,
  },
  /** The ways to save more (the estate's opportunities), largest first. */
  opps: {
    title: 'Opportunities',
    meta: (k: number) => n(k, 'way', 'ways'),
    month: (usd: string) => `${usd}/mo`,
    total: (usd: string) => `Up to ${usd} a month`,
    none: 'Nothing to save right now.',
    label: (title: string, usd: string) => `${title}, saves about ${usd} a month`,
  },
  optimizations: {
    title: 'Optimizations',
    coverage: (k: number, all: number) => `${k} of ${n(all, 'agent', 'agents')}`,
    none: 'No savings reported yet.',
    off: 'Off',
    program: (name: string, usd: string) => `${name}, saved ${usd}`,
    ways: (k: number, usd: number) =>
      `${n(k, 'way', 'ways')} to save ${usd > 0 ? `${usd0(usd)} more` : 'more'}`,
    waysHere: (k: number) => `${n(k, 'way', 'ways')} to save more on this`,
  },
  tool: {
    off: (days: number) =>
      `Turned off ${days < 1 ? 'today' : days < 2 ? 'yesterday' : `${Math.floor(days)} days ago`}`,
    empty: 'No tools here.',
  },
  routerLabel: (name: string, spend: string, share: string) =>
    `${name}, ${spend}, ${share} of agent spend`,
  saved: 'Saved',
  savedSub: 'from optimizations',
  savedLabel: (x: string) => `Saved ${x} from optimizations: open Optimization`,
  opportunities: (k: number) => `${n(k, 'way', 'ways')} to save more`,
  /** A card's title is a button: it lights the card and opens its insights in the drawer. */
  cardLabel: (what: string) => `${what}: show insights`,
  /** The bill's peak days, each a button that opens the day in the drawer. */
  dayLabel: (day: string, usd: string) => `${day}: ${usd}, over the daily line. Show the day`,
  routerTile: (name: string, dead: boolean) => (dead ? `${name}, not answering` : name),
  /** A workflow row's state, where a team row has its pace. */
  flowState: (bad: number, warn: number) =>
    bad
      ? `${n(bad, 'step', 'steps')} failing`
      : warn
        ? `${n(warn, 'step', 'steps')} slow`
        : 'healthy',
  system: {
    title: 'System',
    parts: (k: number) => n(k, 'part', 'parts'),
    search: 'Search teams, agents, providers',
    show: 'Show',
    all: 'All',
    attention: 'Needs attention',
    healthy: 'Healthy',
    org: 'Org',
    fleet: 'Fleet Runtime',
    teams: 'Teams & people',
    harnesses: 'Local harnesses',
    tools: 'Connectors & tools',
    providers: 'Providers',
    models: 'Models',
    hide: 'Fold the system list',
    openLabel: 'System: show every part',
    empty: 'Nothing matches.',
  },
  /** What a person's tile says about how they use the team's budget. */
  peopleKey: { ok: 'getting value', warn: 'expensive', bad: 'stuck', idle: 'light use' },
  /** What a card's insights offer to do next, in the drawer. */
  actions: {
    setBudgets: 'Set budgets',
    deploy: 'Deploy an agent',
    newWorkflow: 'New workflow',
    connect: 'Connect a tool',
    tokenops: 'Open TokenOps',
    optimize: 'Open Optimization',
    agents: 'All agents',
    runs: 'Workflow runs',
    harnesses: 'Open harnesses',
    routers: 'Set up routing',
    providers: 'Add a provider key',
    tools: 'All tools',
    investigate: 'Investigate spend',
    label: (section: string) => `${section} actions`,
  },
  /** A card's insights, in the drawer: what it adds up to and how to read it. */
  detail: {
    teams: (teams: number, over: number, loose: number) =>
      `${n(teams, 'team', 'teams')}, ${over} over pace, ${loose} with no budget.`,
    teamsKey: 'Bar: spent, dashed where the month ends, the tick is the limit',
    agents: (k: number, total: number, failing: number) =>
      `${n(k, 'agent', 'agents')} spent ${usd0(total)}; ${failing} failing today.`,
    agentsKey: 'Big tile: today. Small tiles: the last 18 days, oldest first',
    harness: (installs: number, kinds: number, total: number) =>
      `${n(installs, 'install', 'installs')} of ${n(kinds, 'kind', 'kinds')}, ${usd0(total)} in the range.`,
    harnessKey: 'One seat per install, the busiest first; grey seats are free',
    workflows: (k: number, runs: number, failing: number) =>
      `${n(k, 'workflow', 'workflows')}, ${runs.toLocaleString('en-US')} runs; ${failing} with a failing step.`,
    workflowsKey: 'One segment per step, colored by the agent or tool it calls',
    tools: (k: number, unused: number, off: number) =>
      `${n(k, 'tool', 'tools')}, ${unused} not used by anything${off ? `, ${off} turned off lately` : ''}.`,
    toolsKey:
      'Most used first: agents, coding harnesses and workflow steps that call it. Faded: turned off lately',
    bill: (avg: number, peak: number, line: number | null) =>
      `${usd(avg)} a day on average, ${usd(peak)} at the peak${line !== null ? `; the line is ${usd(line)} a day` : ''}.`,
    billKey:
      'Both sides add up to the total. Dotted on the line: the days that peaked over it; pick one to read it',
    optimizations: (pct: number | null, calls: number, all: number) =>
      `${pct !== null ? `${Math.round(pct)}% less spent` : 'Nothing saved yet'}; ${calls.toLocaleString('en-US')} of ${all.toLocaleString('en-US')} calls optimized.`,
    optimizationsKey: 'Bar: what each program saved. Off: no agent has it turned on',
    budget: (unbudgeted: number, uncapped: number) =>
      `${usd0(unbudgeted)} has no budget; ${usd0(uncapped)} of coding-harness spend can only alert.`,
    budgetKey: 'The tick is the alert; past it the arc turns orange, past the limit red',
    saved: (pct: number | null, calls: number, all: number) =>
      `${pct !== null ? `${Math.round(pct)}% less spent` : 'Nothing saved yet'}; ${calls.toLocaleString('en-US')} of ${all.toLocaleString('en-US')} calls optimized.`,
    savedKey: 'What each program took off the bill in this range',
    opps: (k: number, usd: number) =>
      k
        ? `${n(k, 'way', 'ways')} to save, up to ${usd0(usd)} a month.`
        : 'Nothing to save right now.',
    oppsKey: 'Largest first. Pick one to see what it changes and where',
    paying: 'Hover the bill: every line it pays for turns green; red stays where spend is going up',
    routers: (k: number, routed: number | null) =>
      `${n(k, 'router', 'routers')} carry ${routed === null ? 'no' : pct(routed)} of the agents' spend.`,
    routersKey: 'Pick a router to light its agents, its models and the savings it carries',
    providers: (top: string | null) =>
      top ? `The most spent on: ${top}.` : 'No model has spend in this range.',
    providersKey: 'One bar per model, every column on one scale',
  },
  teams: 'Teams',
  teamsMeta: (teams: number, people: number) =>
    `${n(teams, 'team', 'teams')} · ${n(people, 'person', 'people')}`,
  noTeam: 'No team',
  teamNoBudget: 'No budget',
  pace: (p: number) => `${p > 1 ? '▲ ' : ''}${pct(p)} pace`,
  teamLabel: (name: string, spent: number, limit: number | null, pace: number | null) =>
    limit
      ? `${name}, ${usd0(spent)} of ${usd0(limit)}${pace !== null ? `, on pace for ${pct(pace)}` : ''}`
      : `${name}, ${usd0(spent)}, no budget`,
  /** Under a team's bar: where the month ends, and what the line does. */
  teamNote: (projected: number | null, limit: number | null, hard: boolean) =>
    limit && projected !== null
      ? `On pace for ${usd0(projected)} by month end · ${hard ? 'hard cap' : 'alerts only'}`
      : 'Nothing holds this spend',
  /** The drawer for a card or a day of the bill. */
  panel: {
    label: 'Insights',
    close: 'Close',
    attention: 'Needs a look',
    healthy: 'Nothing here needs a look.',
    programs: 'By program',
    ways: 'Ways to save',
    peaks: 'Peak days',
    teams: 'Teams',
    open: 'Open',
    day: (d: string) => `${d} spend`,
    daySub: (usd: string, line: string | null) =>
      line ? `${usd} spent, over the ${line} daily line.` : `${usd} spent.`,
    spend: 'Spend',
    line: 'Daily line',
    average: 'Average day',
    budgeted: 'In a budget',
    unbudgeted: 'No budget',
    saved: 'Saved',
    top: 'Top agent',
    dayTokenops: 'Open the day in TokenOps',
    daySessions: 'Sessions that day',
    allBill: 'Back to the bill',
  },
  people: (tones: readonly string[]) => {
    const c = (t: string) => tones.filter((x) => x === t).length
    return `${n(tones.length, 'person', 'people')}: ${c('ok')} getting value, ${c('warn')} expensive, ${c('bad')} stuck, ${c('idle')} light use`
  },
  usd,
  usd0,
  /** A card's count in its corner, linking to the page with all of them. */
  count: (shown: number, all: number) => (shown < all ? `${shown} of ${all}` : `${all}`),
  countLabel: (what: string, all: number) => `All ${all} ${what.toLowerCase()}`,
  runs: (k: number) => `${k.toLocaleString('en-US')} runs`,
  uses: (k: number) => n(k, 'use', 'uses'),
  usesTitle: (agents: number, harnesses: number, steps: number) =>
    `Used by ${n(agents, 'agent', 'agents')}, ${n(harnesses, 'coding harness', 'coding harnesses')} and ${n(steps, 'workflow step', 'workflow steps')}`,
  installs: (k: number) => n(k, 'install', 'installs'),
  slots: (k: number) => `${n(k, 'free seat', 'free seats')}: room for the installs still to come`,
  change: (p: number) => `${Math.abs(Math.round(p))}%`,
  changeLabel: (p: number) =>
    `${p <= 0 ? 'Down' : 'Up'} ${Math.abs(Math.round(p))}% on the window before`,
  share: pct,
  noRoutedSpend: 'No routed spend',
  others: 'Other models',
  day: (k: number, state: string) => `${k === 0 ? 'Today' : `${k} days ago`}: ${state}`,
  tone: {
    ok: 'healthy',
    bad: 'failing',
    warn: 'slow or costly',
    idle: 'idle',
    none: 'no data',
  } as Record<string, string>,
  harnessTone: {
    high: 'busy',
    low: 'light use',
    empty: 'no priced use',
    bad: 'failing',
    warn: 'slow or costly',
  } as Record<string, string>,
  legend: {
    spend: 'Spend, as wide as it is',
    hot: 'Spend going up',
    calls: 'Calls',
  },
}
