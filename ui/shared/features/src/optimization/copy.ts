/**
 * Every user-facing string for context optimization (plans/feat-context-optimization.md §3, decision ledger §11).
 * The page is "Optimization", never "Context" (N1); savings are "fewer tokens", never "better answers" (§5).
 */
import type { Level, Strategy } from './types'

const LEVEL_LABEL: Record<Level, string> = { low: 'Low', medium: 'Medium', high: 'High' }

export const copy = {
  title: 'Optimization',
  // The /optimization page (plans/feat-optimization-page.md; P2 moved your settings here from Settings → Account).
  hub: {
    yourSettings: 'Your settings',
    yourSettingsNote: 'now · your chats with agents and the Orchestrator',
    /** R1D: the header's jump to #settings. */
    settingsLink: 'Your settings:',
    off: 'Off',
    /** PanelError's "Couldn’t load …" when your account can't be read. */
    account: 'your account',
    settingsLinkLabel: (summary: string) => `Your settings: ${summary}. Go to your settings`,
  },

  // Needs attention (P5, R1C, R2C, R2F; eng E2): only what you can fix, split by whose chats it affects.
  attention: {
    title: 'Needs attention',
    now: 'now',
    failedCheck: 'Couldn’t check your agents.',
    mine: (n: number, serverOff: boolean) =>
      `${n} of your agents ${n === 1 ? 'has' : 'have'} Token optimization off${serverOff ? '.' : `, so ${n === 1 ? 'its' : 'their'} chats aren’t optimized.`}`,
    others: (n: number) =>
      `${n} other ${n === 1 ? 'agent' : 'agents'} in the workspace ${n === 1 ? 'has' : 'have'} it off, so ${n === 1 ? 'its' : 'their'} chats aren’t optimized.`,
    more: (n: number) => `and ${n} more`,
    turnOnMine: (n: number) => (n === 1 ? 'Turn it on' : `Turn on for your ${n}`),
    turnOnAll: (n: number) => (n === 1 ? 'Turn it on' : `Turn on for all ${n}`),
    progress: (done: number, total: number) => `Turning on ${done} of ${total}…`,
    confirmTitle: (n: number) =>
      `Turn on Token optimization for ${n} ${n === 1 ? 'agent' : 'agents'}?`,
    confirmMine:
      'Turns on compression and a concise-answer instruction for these agents’ requests and replies, in every chat that uses them. Each switch affects only its own agent.',
    confirmOthers:
      'These agents belong to other people. Turning it on compresses these agents’ requests and replies in every chat that uses them, including other people’s.',
    confirm: (n: number) => (n === 1 ? 'Turn on' : `Turn on for ${n}`),
    owner: (name: string) => `owner ${name}`,
    failed: (name: string, reason: string) => `${name}: ${reason}`,
    failedFallback: 'couldn’t save',
    retryFailed: 'Retry',
    announce: (ok: number, failed: number) =>
      [
        ok ? `Turned on Token optimization for ${ok} ${ok === 1 ? 'agent' : 'agents'}.` : null,
        failed ? `${failed} ${failed === 1 ? 'agent' : 'agents'} couldn’t be turned on.` : null,
      ]
        .filter(Boolean)
        .join(' '),
  },

  // The Workspace footer (R3A, R5B; §7): superusers only.
  workspace: {
    title: 'Workspace',
    tiers: 'Optimization tiers',
    /** CX-T1 absent: the server's built-in values, said as defaults (2B). */
    defaults: 'Optimization tiers (defaults)',
    tierValues: (low: number, medium: number, high: number) =>
      `Low ${low.toLocaleString('en-US')} · Medium ${medium.toLocaleString('en-US')} · High ${high.toLocaleString('en-US')} tokens`,
    tiersFailed: 'Couldn’t load tiers.',
    editTiers: 'Edit tiers',
  },

  // What's optimizing your tokens (plans/feat-optimization-page.md §11, B12): each mechanism, what it trims, where it's
  // set and its state.
  /** What the team names mean, in the Codename tooltip (B12; user, 2026-10-05). */
  codenames: {
    pacms:
      'PACMS keeps the earlier messages most relevant to what you just asked, plus the newest few, within your budget. Top-K keeps the closest matches; Last-K keeps the most recent.',
  },

  mechanisms: {
    title: 'What’s optimizing your tokens',
    history: 'History selection (PACMS, Top-K or Last-K)',
    historyTrims: 'Trims the chat history kept with each message.',
    historyWhere: 'Your settings, below',
    tiersWhere: 'Optimization tiers',
    tiersNote: 'tier sizes for everyone:',
    caveman: 'Smaller prompts',
    cavemanTrims: 'Trims tool output, chat history and answer length.',
    cavemanWhere: 'Token optimization, on each agent’s Settings tab',
    ponytail: 'Less code written',
    ponytailTrims: 'Coding agents check for an existing solution before writing code.',
    ponytailWhere: 'Minimal-code mode, on a coding agent’s Settings tab',
    comments: 'Prompt comments',
    commentsTrims: 'Agents record, prune and maintain their own workspace instructions.',
    commentsWhere: 'Prompt comments, on each agent’s Settings tab',
    setIn: 'Set in',
    yourAgents: 'Your agents',
    off: 'Off',
    agentsOn: (on: number, total?: number) =>
      total === undefined
        ? `${on.toLocaleString('en-US')} ${on === 1 ? 'agent' : 'agents'} on in the workspace`
        : `${on.toLocaleString('en-US')} of ${total.toLocaleString('en-US')} workspace agents on`,
    saved: (tokens: string, estimated: boolean) =>
      `~${tokens} tokens saved${estimated ? ' (estimated)' : ''}`,
    absent: 'Counts and savings for each show here once your OpenRuntime server reports them.',
    failed: 'Couldn’t load counts and savings.',
  },

  // By agent (R1C, R4A, R6A, R7B) and Biggest senders (R1C; eng C4, C5).
  byAgent: {
    title: 'By agent',
    colAgent: 'Agent',
    colRequests: 'Requests',
    colWithout: 'Without',
    colSent: 'Sent',
    colSaved: 'Saved',
    colSwitch: 'Token optimization',
    on: 'On',
    off: 'Off',
    /** R7B, as sorted (review: design). */
    caption: (
      shown: number,
      total: number,
      by: 'without' | 'sent' | 'saved' | 'requests' | 'name',
    ) =>
      by === 'name'
        ? `First ${shown} of ${total} by name.`
        : `Top ${shown} of ${total} by ${{ without: 'history volume', sent: 'tokens sent', saved: 'tokens saved', requests: 'requests' }[by]}.`,
    showAll: (n: number) => `Show all ${n}`,
    showTop: 'Show fewer',
    sortBy: (col: string) => `Sort by ${col}`,
    phoneLine: (saved: string, pct: number, without: string) =>
      `~${saved} saved · ${pct}% of ~${without}`,
    outline: 'Each agent’s history tokens, sent and saved, will show here.',
    empty: 'No agent reported in this window.',
    what: 'savings by agent',
  },
  senders: {
    title: 'Biggest senders',
    perMessage: 'per message',
    otherChat: 'Another user’s chat',
    untitled: 'Untitled chat',
    row: (without: string, sent: string) => `~${without} without → ~${sent} sent`,
    off: 'Token optimization off',
    /** The request's time, UTC like the chart's buckets. */
    at: (time: string) => `${time} UTC`,
    outline: 'The chats that send the most history will show here.',
    empty: 'No requests reported in this window.',
    emptySlice: (label: string) => `No requests reported in ${label}.`,
    what: 'biggest senders',
  },

  // The lead: the answer heading, the five-number sentence and the trend chart (R1A, R2A, R4B, R5A; eng C1–C3, C5, C6).
  lead: {
    heading: {
      error: 'Couldn’t load savings',
      absent: 'This server doesn’t report savings yet',
      noAgents: 'You don’t have access to any agents yet',
      noReports: 'No requests reported yet',
      lowCoverage: 'Too few requests reported to estimate',
      saving: (pct: number) => `Saving ~${pct}% of history tokens`,
      none: 'No history tokens saved in this window',
    },
    /** R7A: what the numbers cover. */
    meta: (period: string, superuser: boolean) =>
      `${period} · ${superuser ? 'all agents' : 'agents you can access'} · estimates`,
    absentLine:
      'Your OpenRuntime server doesn’t record what each request carried yet. Every block below says what will show there once it does.',
    noReportsLine: 'Chat with an agent, then come back: each request’s savings show here.',
    startChat: 'Start a chat',
    noAgentsLine:
      'Once someone shares an agent with you, or you deploy one, its savings show here.',
    /** R4B: sent, would-have-sent, saved, cost and %. The % is left out under 50% coverage (C2). */
    sentence: (
      superuser: boolean,
      sent: string,
      without: string,
      saved: string,
      cost: string | null,
      pct: number | null,
    ) => {
      const extra = [cost ? `≈ ${cost}` : null, pct !== null ? `${pct}%` : null].filter(Boolean)
      return `${superuser ? 'All agents' : 'Agents you can access'} sent ~${sent} tokens where they would have sent ~${without}: ~${saved} fewer${extra.length ? ` (${extra.join(', ')})` : ''}.`
    },
    /** The lead's four figures (redesign 2026-10-13); the baseline label is `preview.without`. */
    figures: {
      notBilled: 'Input not billed',
      fewer: (pct: number) => `${pct}% fewer tokens`,
      reported: (pct: number) => `${pct}% of requests reported`,
    },
    /** C2. */
    basedOn: (reports: number, eligible: number, pct: number) =>
      `Based on ${reports.toLocaleString('en-US')} of ${eligible.toLocaleString('en-US')} requests (${pct}%).`,
    /** C3. */
    costCovers: (priced: number, reports: number) =>
      `Cost covers ${priced.toLocaleString('en-US')} of ${reports.toLocaleString('en-US')} requests.`,
    /** C6. */
    trend: (direction: 'up' | 'down', from: number, to: number, days: number) =>
      `${direction === 'up' ? 'Up' : 'Down'} from ~${from}% to ~${to}% of history tokens saved over the last ${days} recorded days.`,
    /** CX-H, described, never as a cause (C6). */
    change: (field: 'strategy' | 'level' | 'enabled', to: string, day: string) =>
      field === 'strategy'
        ? `You switched to ${to} on ${day}.`
        : field === 'level'
          ? `You set your history budget to ${to} on ${day}.`
          : `You turned context optimization ${to === 'true' ? 'on' : 'off'} on ${day}.`,
    tokenops: 'Spend for this window → TokenOps',
    /**
     * C1, P10: history compression for your chats, from `compressionState`. "some-off" isn't said here: Needs attention
     * names those agents and fixes them.
     */
    compression: {
      'all-on': 'History compression is on for your Orchestrator chats.',
      'likely-on':
        'History compression is likely on for your Orchestrator chats: the server’s own setting can still turn it off.',
      'server-off': 'History compression is off on this server.',
      'no-agents':
        'History compression is on for your Orchestrator chats unless the server turns it off.',
    },
    chart: {
      sent: 'Sent',
      saved: 'Saved (not sent)',
      full: 'Full bar =',
      fullWithout: 'without optimization',
      notRecorded: 'Not recorded',
      noRequests: 'No requests',
      table: 'Table',
      chart: 'Chart',
      bucket: 'Bucket',
      without: 'Without',
      savedCol: 'Saved',
      reported: 'Reported',
      open: 'Show requests',
      actionCol: 'Action',
      lowCoverage: (reports: number, eligible: number) =>
        `Too few reported (${reports.toLocaleString('en-US')} of ${eligible.toLocaleString('en-US')})`,
      /** R4C: the baseline is defined once, here. */
      footnote: (unit: 'hour' | 'day' | 'week') =>
        `Without optimization = the recent messages OpenRuntime chooses from (up to 150), before selection. Fewer tokens, not necessarily better answers. Click ${unit === 'hour' ? 'an hour' : `a ${unit}`} to see its biggest senders.`,
      /** R6B: a phone's chart bar is a week. */
      week: (label: string) => `Week of ${label}`,
      outline: (hourly: boolean) =>
        `Tokens sent and saved per ${hourly ? 'hour' : 'day'} will show here once your OpenRuntime server reports them.`,
      summary: (period: string, sent: string, saved: string) =>
        `Tokens over ${period}: ~${sent} sent, ~${saved} saved.`,
      loading: 'Loading savings over time',
      what: 'savings over time',
    },
    slice: {
      selected: (label: string) => `${label} selected`,
      clearLabel: (label: string) => `Clear ${label}`,
    },
  },
  /** The intro when the server has no off switch (CX-5 absent): the switch section is hidden (1A). */
  subAlwaysOn:
    'Context optimization is always on; choose how much past conversation goes with each new message, in chats with agents and with the Orchestrator. Workflows don’t use this.',
  section: (n: number, title: string) => `${n}. ${title}`,

  switchTitle: 'Context optimization',
  switchHint: 'On for every chat. Off sends recent history as-is (most tokens).',
  switchOffNote: 'Off: strategy and budget don’t apply until you turn it back on.',

  strategyTitle: 'Context strategy',
  strategyHint: 'Which past messages go with each new one.',
  strategies: {
    pacms: {
      label: 'PACMS · Recommended',
      name: 'PACMS',
      hint: 'Picks the most useful past messages that fit your budget. Uses embeddings.',
    },
    topk: {
      label: 'Top-K',
      name: 'Top-K',
      hint: 'Most relevant question–answer pairs. Uses embeddings.',
    },
    lastk: {
      label: 'Last-K',
      name: 'Last-K',
      hint: 'Only the most recent messages. Fastest, no embeddings.',
    },
  } satisfies Record<Strategy, { label: string; name: string; hint: string }>,

  budgetTitle: 'History budget',
  budgetHint: (strategy: Strategy) =>
    strategy === 'pacms'
      ? 'A token budget for past messages under PACMS.'
      : strategy === 'topk'
        ? 'How many question–answer pairs Top-K keeps.'
        : 'How many recent messages Last-K keeps.',
  level: (l: Level) => LEVEL_LABEL[l],
  /** 2B: only with the server's tier values. */
  figure: (l: Level, kind: 'tokens' | 'pairs' | 'messages', value: number) =>
    `${LEVEL_LABEL[l]} ≈ ${value.toLocaleString('en-US')} ${kind === 'pairs' && value === 1 ? 'pair' : kind === 'messages' && value === 1 ? 'message' : kind}.`,

  preview: {
    caption: (messages: number, strategy: Strategy) =>
      `On your last chat (${messages.toLocaleString('en-US')} messages) · with ${copy.strategies[strategy].name}`,
    tier: 'Tier',
    kept: 'Messages kept',
    keptShort: 'Kept · tokens',
    tokens: 'Tokens sent',
    current: '· current',
    without: 'Without optimization',
    /** 2A: one baseline on every surface. */
    baselineTip:
      'The recent messages OpenRuntime chooses from (up to 150), before selection. Chats longer than 150 messages count as 150.',
    /** 2A on every device: tooltips don't open on tap, so the definition is in the footnote too (review finding 3). */
    estimates:
      'Estimates. Without optimization = the recent messages OpenRuntime chooses from (up to 150), before selection. Fewer tokens, not necessarily better answers.',
    noChats: 'The preview appears after your first chat.',
    failed: 'Preview unavailable.',
    label: 'What each tier keeps from your last chat',
  },

  // The trace's Optimization block (design review 1B, 2A, 5A, 6A).
  trace: {
    title: 'Optimization',
    with: 'With',
    withDetail: (level: string | null, strategy: string | null) =>
      [level, strategy].filter(Boolean).join(' · '),
    messages: 'Messages',
    tokens: 'Tokens',
    compressed: 'Compressed',
    policy: 'Policy',
    policyApplied: 'Organization policy applied',
    pointer: 'Optimization for this request:',
    label: 'What this request carried',
    measure: 'Measure',
  },

  // The lead's coverage footnote (2H) and TokenOps' link to this page (P3).
  savings: {
    coverage: (date: string) => `Recorded since ${date}; earlier days aren’t counted.`,
    /** P3: TokenOps' link here; the trend, the agents and the fixes live on the Optimization page. */
    more: 'Trend, agents and fixes → Optimization',
  },

  // Settings → Workspace → Optimization tiers (§4; design review 2B; sketch v2 5).
  tiers: {
    title: 'Optimization tiers',
    sub: 'What Low, Medium and High mean for everyone’s chats. People pick a tier in Your settings on the Optimization page.',
    defaultsTitle: 'Defaults. Your server’s environment may override these.',
    partialTitle: 'Read-only. Your server reports only some of these values.',
    partialHint:
      'Values it doesn’t report show their defaults. Editing needs a newer OpenRuntime server that reports every value.',
    conflict: 'Someone else saved the tiers after you opened this page. Your changes aren’t saved.',
    conflictAction: 'Load their values',
    conflictLoadFailed: 'Couldn’t load their values. Your changes are still here; try again.',
    defaultsHint:
      'They are read when OpenRuntime starts. Editing them here needs a newer OpenRuntime server; until then, change the variables and restart.',
    budgets: 'History budgets (PACMS)',
    counts: 'Message counts (Top-K and Last-K)',
    shared: 'Shared limits',
    budgetHint: {
      low: 'Token budget for past messages.',
      medium: 'The default tier.',
      high: 'Most history, most tokens.',
    },
    countHint: { low: 'Pairs or messages kept.', medium: '', high: '' },
    pool: 'History pool',
    poolHint: 'How many recent messages PACMS chooses from.',
    kept: 'Always kept',
    keptHint: 'The newest messages, kept whatever the budget.',
    compressOver: 'Compress history over',
    compressOverHint: 'Longer history is compressed when the owner’s agents allow it.',
    compressHistory: 'History compression',
    compressHistoryHint: 'Server-wide. Off turns history compression off for everyone.',
    unit: { tokens: 'tokens', messages: 'messages', bytes: 'bytes' },
    value: (n: number, unit: string) =>
      `${n.toLocaleString('en-US')} ${n === 1 ? unit.replace(/s$/, '') : unit}`,
    on: 'On',
    off: 'Off',
    problem: {
      whole: 'A whole number, 1 or more.',
      order: 'Must not be lower than the tier before it.',
      pool: 'Must be at least the always-kept messages.',
    },
    saved: 'Optimization tiers saved. They apply to the next message.',
    loading: 'Loading optimization tiers',
    what: 'optimization tiers',
  },

  save: 'Save changes',
  saving: 'Saving…',
  /** R2G: beside Save while the bar sticks. */
  unsaved: 'Unsaved changes',
  saved: (fields: string[]) => `${fields.join(' and ')} saved. Applies from your next message.`,
  fieldName: { strategy: 'Strategy', level: 'History budget', enabled: 'Context optimization' },
  fieldFailed: (field: string, message: string) =>
    `${field} didn’t save: ${message}. Save changes to try again.`,
  saveFailed: 'OpenRuntime couldn’t save this',
  loading: 'Loading your optimization settings',
  retry: 'Retry',
}
