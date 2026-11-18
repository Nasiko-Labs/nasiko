/**
 * Every user-facing Chat string (plan §2 vocabulary). Error entries always carry a problem,
 * a cause and an action (errors.test.ts enforces it).
 */

export interface ErrorCopy {
  problem: string
  cause: string
  action: string
}

export const errorCopy = {
  createFailed: {
    problem: 'Not sent.',
    cause: "Open Runtime couldn't start this chat.",
    action: 'Try again.',
  },
  saveUserFailed: {
    problem: "Couldn't save your message.",
    cause: 'Open Runtime refused or lost the request, so nothing was sent to the agent.',
    action: 'Try again.',
  },
  badRequest: {
    problem: "The agent couldn't read this message.",
    cause: 'Open Runtime rejected the request before running it.',
    action: 'Edit the message and send it again.',
  },
  // The agent's own HTTP server refused the body — not an Open Runtime limit, so the composer
  // accepted the file and the dispatch still bounced. Attachments are sent base64'd inside the
  // message, which is what pushes a request past an agent's own cap (§7.7).
  messageTooLarge: {
    problem: 'The agent refused this message as too large.',
    cause: 'Its server caps how big one request can be, and the attachment is over that cap.',
    action: 'Remove the attachment or send a smaller one.',
  },
  forbidden: {
    problem: "You can't use this agent.",
    cause: "Your account doesn't have access to it.",
    action: 'Ask its owner for access, or pick another agent.',
  },
  agentGone: {
    problem: 'This agent is gone or you lost access.',
    cause: 'Open Runtime no longer finds it for your account.',
    action: 'Pick another agent on Agents.',
  },
  rateLimited: {
    problem: 'Too many messages. Try again in a minute.',
    cause: 'Open Runtime allows 30 chat requests per minute per person.',
    action: 'Wait a minute, then retry.',
  },
  noAgents: {
    problem: 'Open Runtime has no running agent for this.',
    cause: 'The agent is stopped or still deploying.',
    action: 'Open Agents to start it.',
  },
  // A 503 that isn't "not running" (an unreadable attachment, a refused session): its reason shows under it.
  refusedWithReason: {
    problem: "Open Runtime couldn't run this message.",
    cause: 'It refused the request before the agent saw it.',
    action: 'Fix what the reason says, then send it again.',
  },
  // A 5xx that came with a reason: the server answered, so the connection didn't drop.
  serverError: {
    problem: "Open Runtime couldn't finish this request.",
    cause: 'It answered with an error, and the agent may already have started.',
    action: 'Fix what the reason says, then refresh status before running it again.',
  },
  cutOff: {
    problem: 'The reply was cut off.',
    cause: 'The connection ended before the agent finished. It may still be running.',
    action: 'Refresh status to check for a saved reply.',
  },
  unreadable: {
    problem: "The agent sent a reply we couldn't read.",
    cause: "Its stream contained events that aren't valid A2A.",
    action: 'Open the trace or the agent logs.',
  },
  tooLarge: {
    problem: 'The reply was too large to show.',
    cause: 'It passed the 5 MB limit for one reply.',
    action: 'Open the trace to see what the agent returned.',
  },
  agentFailed: {
    problem: 'The agent reported an error.',
    cause: 'It ended this turn as failed.',
    action: 'Open the agent logs, then try again.',
  },
  noReply: {
    problem: 'No reply has been saved for this message yet.',
    cause: 'The agent finished without text, or the reply was lost when the stream ended.',
    action: 'Refresh status, or run it again.',
  },
  saveDefinite: {
    problem: 'Reply not saved.',
    cause: 'Open Runtime refused to store it.',
    action: 'Save again, or discard it.',
  },
  saveUnknown: {
    problem: 'This reply may already be saved.',
    cause: 'The save request failed without an answer from Open Runtime.',
    action: 'Refresh status before saving again.',
  },
  busy: {
    problem: 'This chat is still starting a reply.',
    cause: 'An answer or a re-run for this chat has not started streaming yet.',
    action: 'Wait a moment, then send again.',
  },
  otherTab: {
    problem: 'Running in another tab.',
    cause: 'This chat already has a message in progress in another tab.',
    action: 'Wait for it there, or switch to that tab.',
  },
  tooLong: {
    problem: 'Message is too long.',
    cause: 'One message can be up to 100,000 characters.',
    action: 'Shorten it or split it into parts.',
  },
  // Routed chats (v1b §5.6): chosen by chatMode; direct chats keep the entries above.
  routedBadRequest: {
    problem: "The Orchestrator couldn't read this request.",
    cause: 'It rejected the request before running it.',
    action: 'Edit the message and send it again.',
  },
  routedForbidden: {
    problem: 'The Orchestrator refused this request.',
    cause: "Your account isn't allowed to run it.",
    action: 'Ask an admin for access.',
  },
  routedReconnectForbidden: {
    problem: "This reply was started by another sign-in and can't be resumed here.",
    cause: 'The paused reply belongs to another sign-in.',
    action: 'Copy the details, then start a new chat.',
  },
  // An answered routed request with no reply saved yet (ship review D3): never Run again.
  routedAnsweredNoReply: {
    problem: 'No reply was saved after your answer.',
    cause:
      'The agent may already have acted on your answer, so sending the message again could repeat it.',
    action: 'Refresh status, or send a new message.',
  },
  // The agent lost the paused task (hitl resume_status agent_lost_task): nothing ran, asking again is safe.
  routedAgentLostTask: {
    problem: 'Your answer could not be delivered.',
    cause: 'The agent no longer had the paused task, so it never received your answer.',
    action: 'Send your message again to start over.',
  },
  routedMayStillArrive: {
    problem: 'The reply may still be arriving.',
    cause: "Open Runtime couldn't replay the resumed reply.",
    action: 'Refresh status.',
  },
  routedRateLimited: {
    problem: 'Too many requests.',
    cause: 'Limit is 30 requests a minute.',
    action: 'Wait a minute and try again.',
  },
  routedInternal: {
    problem: 'Open Runtime hit an error.',
    cause: 'The request may have started.',
    action: 'Refresh status before running it again.',
  },
  routedNoAgents: {
    problem: 'No accessible, reachable agents were available for this request.',
    cause: 'None of your agents is running or reachable.',
    action: 'Check access and that agents are running.',
  },
  routedFailed: {
    problem: 'The Orchestrator reported an error.',
    cause: 'It ended this turn as failed.',
    action: 'Open the trace, then run it again.',
  },
  // The Waiting queue's poll (v1c §5.9, DX4).
  pendingFailed: {
    problem: "Couldn't load waiting requests",
    cause: "Open Runtime didn't answer the pending-requests check.",
    action: 'Retry, or check the server logs with the correlation id.',
  },
  tooManyLive: {
    problem: 'Three replies are already in progress.',
    cause: 'A browser tab can keep only a few replies streaming at once.',
    action: 'Wait for one to finish.',
  },
  // The attachments failed to upload, so nothing was sent: the message is still in the box.
  uploadFailed: {
    problem: "Couldn't upload your attachments.",
    cause: 'Open Runtime refused or lost the upload, so nothing was sent to the agent.',
    action: 'Try again, or remove the attachment and send the message without it.',
  },
  // Dictation (docs/superpowers/specs/2026-11-13-chat-dictation-design.md). The draft is never touched.
  // `navigator.mediaDevices` is undefined on an insecure origin, usually Open Runtime served over plain HTTP on an IP.
  micInsecure: {
    problem: "Couldn't use the microphone.",
    cause: 'Browsers allow microphones only on HTTPS pages and localhost.',
    action: 'Open Open Runtime over HTTPS, or ask your admin to set it up.',
  },
  micDenied: {
    problem: "Couldn't use the microphone.",
    cause: 'Microphone access is blocked for this site.',
    action: 'Allow it in your browser’s site settings, then try again.',
  },
  micMissing: {
    problem: "Couldn't use the microphone.",
    cause: 'No microphone was found on this device.',
    action: 'Connect one, then try again.',
  },
  micFailed: {
    problem: "Couldn't start recording.",
    cause: 'The browser refused to record from the microphone.',
    action: 'Try again, or try another browser.',
  },
  transcribeUnconfigured: {
    problem: "Couldn't transcribe your recording.",
    cause: 'Speech-to-text needs an OpenAI API key, and this Open Runtime has none.',
    action: 'Save an OpenAI key in the LLM router, or ask your admin to set OPENAI_API_KEY.',
  },
  // quirk: the server sends a provider failure as a 502 with a body; apiFetch's network error is a 502 without one.
  transcribeRejected: {
    problem: "Couldn't transcribe your recording.",
    cause: 'The transcription provider refused it.',
    action:
      'Try again. If it keeps failing, ask your admin to check OPENAI_BASE_URL supports audio.',
  },
  transcribeUnreadable: {
    problem: "Couldn't transcribe your recording.",
    cause: "Open Runtime couldn't read it. It may be too long.",
    action: 'Record a shorter message.',
  },
  transcribeFailed: {
    problem: "Couldn't transcribe your recording.",
    cause: "Open Runtime didn't answer.",
    action: 'Try again.',
  },
} satisfies Record<string, ErrorCopy>

/** `policy_rejected` copy per FlowGuard limit (v1b §5.6). */
export const policyCopy = {
  depth: 'call depth',
  cycle: 'a call cycle',
  fanOut: 'fan-out',
  tokens: 'token budget',
  timeout: 'time limit',
  guard: 'limit check',
  stopped: (limit: string) => `Stopped by an Open Runtime limit: ${limit}.`,
  raise: (env: string) => `Ask your admin to raise ${env}.`,
  cycleFix: 'The request asked an agent that was already in the call chain.',
  guardFix: "Open Runtime couldn't check its limits. Try again later.",
} as const

export type ErrorKey = keyof typeof errorCopy

export const copy = {
  receivingStopped: 'Receiving stopped. The agent may still be running tools.',
  // Attachments (§7.7). `attach` names the control; the rejection lines each say which file and why.
  attach: 'Attach files',
  attachHint: (max: number, media: string, text: string, other: string) =>
    `Up to ${max} files: images and PDFs ${media}, text files ${text}, other files ${other} each`,
  attachKinds: { image: 'images', pdf: 'PDFs', text: 'text files', other: 'other files' },
  attachedCount: (n: number) => `${n} attachment${n === 1 ? '' : 's'}`,
  removeAttachment: (name: string) => `Remove ${name}`,
  attachDrop: 'Drop files to attach',
  uploading: 'Uploading attachments…',
  attachTooMany: (name: string, max: number) =>
    `${name} wasn't attached: a message can carry ${max} files.`,
  attachTooLarge: (name: string, kind: string, size: string) =>
    `${name} wasn't attached: ${kind} must be under ${size}.`,
  attachOverBudget: (name: string, size: string) =>
    `${name} wasn't attached: one message can carry ${size} of files in total.`,
  attachEmpty: (name: string) => `${name} wasn't attached: it is empty, or it is a folder.`,
  // Dictation: the mic's names per state, and what the live region says.
  startDictation: 'Start dictation',
  // Fixed, so a screen reader doesn't re-read the button every second; the elapsed time beside it is visual only.
  stopDictation: 'Stop dictation',
  transcribing: 'Transcribing…',
  dictationHint: (altR: string) => `Dictate (F8 or ${altR}). Esc discards.`,
  dictationListening: 'Recording. Press F8 to stop, Esc to discard.',
  dictationDiscarded: 'Recording discarded.',
  dictationInserted: 'Transcript added to the message box.',
  dictationEmpty: 'No speech was heard.',
  checkingForReply: 'Checking for a reply…',
  runAgainConfirmTitle: 'Run this message again?',
  runAgainConfirmBody:
    'The agent may already have taken actions (tools, connectors), and they may run twice.',
  sendWhileUnknownTitle: 'Your last message may still be running.',
  sendWhileUnknownBody: 'Send anyway? If it is still running, both may take actions.',
  idleNotice: 'No update for a while. The agent may still be working.',
  refreshStatus: 'Refresh status',
  runAgain: 'Run again',
  newChat: 'New chat',
  chats: 'Chats',
  filterChats: 'Filter chats',
  noChats: 'No chats yet.',
  noChatsHint: 'Pick an agent to start one.',
  loadMore: 'Load more',
  chatsFailed: "Couldn't load your chats.",
  retry: 'Retry',
  chooseTitle: 'Choose an agent',
  chooseHint: 'Chat talks to one agent at a time.',
  allAgents: 'All agents →',
  noRunningAgents: 'No agent is running right now.',
  noRunningAgentsHint: 'Start one from its page on Agents, then come back.',
  askAgent: (name: string) => `Ask ${name}`,
  recentWithAgent: 'Pick up where you left off',
  targetInvalid: (v: string) => `'${v}' isn't an agent you can use. Pick one on Agents.`,
  targetAmbiguous: (v: string) => `Several agents are called '${v}'. Pick one:`,
  targetNotRunning: (name: string) => `${name} isn't running, so it can't reply.`,
  stoppedByOwner: (name: string) =>
    `${name} was stopped by its owner. You can chat with it again once they start it.`,
  targetNotRunningAction: 'Open the agent',
  agentRemoved: "This chat's agent was removed.",
  recordedReadOnly: 'Recorded from a coding harness.',
  viewInHarnesses: 'View in Coding harnesses →',
  loadingChat: 'Loading chat…',
  historyMissing: "This chat was deleted or isn't yours.",
  historyFailed: "Couldn't load this chat.",
  olderFailed: "Couldn't load older messages.",
  loadOlder: 'Load older messages',
  earlierMessages: 'Earlier messages',
  jumpToLatest: 'Jump to latest',
  viewSession: 'View session',
  rename: 'Rename',
  exportMarkdown: 'Export as Markdown',
  delete: 'Delete',
  deleteTitle: 'Delete this chat?',
  deleteBody:
    'It also disappears from Sessions and loses its trace links. A reply that is still running will be lost.',
  deleteBlocked: "Chats with approval history can't be deleted yet.",
  deleteFailed: "Couldn't delete this chat.",
  cancel: 'Cancel',
  titleLabel: 'Chat title',
  titlePlaceholder: 'Name this chat',
  titleEmpty: 'A title needs at least one character.',
  titleCounter: (n: number, max: number) => `${n}/${max}`,
  composerLabel: (agent?: string) => (agent ? `Message ${agent}` : 'Message'),
  composerHint: 'Enter to send, Shift+Enter for a new line',
  send: 'Send',
  tooLongInline: 'Message is too long. One message can be up to 100,000 characters.',
  lockedUnsaved: 'Save or discard the reply above before sending.',
  lockedChecking: 'Waiting to see whether the last message got a reply.',
  lockedResumeArriving:
    'The reply to your answer may still be arriving. Refresh status to check, or start a new chat.',
  resumeWaiting: 'Waiting for the reply to your answer…',
  lockedLoading: 'Loading chat…',
  replaceDraftBody: 'The example replaces what you typed.',
  replace: 'Replace',
  chatsNav: 'Chat',
  moreChatActions: 'More chat actions',
  /** The router, as a routed chat names it (v1c §5.1: "Open Runtime" stays the platform's name). */
  orchestratorName: 'Orchestrator',
  /** "Ask the Orchestrator": composed strings take the article (v1c §5.1). */
  askOrchestrator: 'Ask the Orchestrator',
  /** The router inside a sentence ("Waiting for the Orchestrator…", "Reply from the Orchestrator complete"). */
  theOrchestrator: 'the Orchestrator',
  routedBadgeTooltip: 'The Orchestrator picks which agents to use for each message',
  agentsFailed: "Couldn't load agents",
  // scaffolding: remove when the server routed reply tap ships (rec 1)
  routedHint: 'You can move around the app. Closing or reloading this tab may lose the reply.',
  statusWorking: 'Working on your request…',
  statusAsking: (agent: string) => `Asking ${agent}…`,
  statusWriting: 'Writing the reply…',
  answeredBy: 'Answered by the Orchestrator',
  answeredByUsing: (agents: string) => `Answered by the Orchestrator, using ${agents}`,
  activity: (n: number) => (n === 1 ? 'Activity · 1 agent' : `Activity · ${n} agents`),
  activityFailed: (n: number) => `${n} failed`,
  agentsUsedLabel: 'Agents used',
  summaryRunning: 'Running',
  summaryCompleted: 'Completed',
  summaryFailed: 'Failed',
  summaryMixed: 'Mixed results',
  summaryDidntFinish: "Didn't finish",
  totalCallTime: 'Total call time',
  agentDetailsUnavailable: 'Agent details unavailable',
  openCurrentAgent: 'Open current agent',
  currentAgentTooltip: (name: string) => `Current agent named ${name}`,
  chatWith: (name: string) => `Chat with ${name}`,
  finishedWithoutReply: 'The Orchestrator finished without a reply.',
  partialUnconfirmed: 'Partial reply — completion unconfirmed',
  // scaffolding: remove when the server routed reply tap ships (rec 1)
  replyTooLarge: 'Reply too large to show here; it will appear once saved',
  loadingSavedReply: 'Loading the saved reply…',
  savedReplyFailed: "Couldn't load the saved reply",
  chatDetailsMissing: "Couldn't find this chat's details",
  copyDetails: 'Copy details',
  announceNoReply: 'The Orchestrator finished without a reply',
  announceUnconfirmed: 'Reply may be incomplete',
  announceSavedFailed: "Couldn't load the saved reply",
  unknownScenario: (x: string, valid: string) =>
    `Unknown mock scenario '${x}'. Using the default. Valid: ${valid}`,
  recordedBadge: 'Recorded',
  // Chat identity (v1c §5.2): the rail row, the header and the composer target.
  identityAgent: 'Agent',
  identityAgentSubline: (status: string) => `Agent · ${status}`,
  identityOrchestratorSubline: 'Picks agents for each message',
  identityRecordedSubline: 'Recorded from a coding harness',
  identityRemoved: 'Agent removed',
  identityRemovedSubline: 'This agent no longer exists',
  // The rail (v1c §5.3).
  searchChats: 'Search chats',
  groupToday: 'Today',
  groupYesterday: 'Yesterday',
  groupWeek: 'Previous 7 days',
  groupOlder: 'Older',
  noLoadedMatch: (q: string) => `No loaded chats match '${q}'`,
  loadMoreToSearch: 'Load more to search older chats',
  noMatchHint: 'Try another word, or clear the filter.',
  // New chat (v1c §5.4).
  heroChooseTitle: 'Start a chat',
  heroChooseSubline: 'Pick an agent, or let the Orchestrator choose.',
  heroAgentSubline: 'Ask me anything',
  heroDescriptionMore: 'More',
  heroDescriptionLess: 'Less',
  chooseAnother: 'Choose another agent',
  heroOrchestratorTitle: 'Orchestrate a task',
  heroOrchestratorSubline: 'Describe a task and the Orchestrator picks the agents to run it.',
  chooseTarget: 'Choose where to send',
  sendTo: 'Send to',
  needTarget: 'Choose an agent or the Orchestrator first',
  stoppedTarget: (name: string) => `${name} isn't running. Start it on Agents.`,
  noAgentsMatch: (q: string) => `No agents match '${q}'`,
  replacedDraft: 'Replaced your saved draft',
  undo: 'Undo',
  composerPlaceholder: 'Message…',
  // Rail views (v1c §5.7, §5.9, DS9-DS12).
  railViews: 'Chat views',
  viewChats: 'Chats',
  viewRecorded: (n: string) => `Recorded (${n})`,
  viewRecordedShort: 'Recorded',
  noRecorded: 'No recorded sessions yet',
  noRecordedHint: 'Coding-harness sessions show here, read-only, once a harness records one.',
  noLiveLoaded: 'No live chats in the loaded list',
  // Recorded harness chats (v1c §5.7).
  toolCallsSummary: (n: number, failed: number) =>
    failed ? `${n} tool calls · ${failed} failed` : `${n} tool calls`,
  moreFailed: (n: number) => `+${n} more failed`,
  showAllCalls: (n: number) => `Show all ${n}`,
  recordedFailed: 'failed',
  recordedDenied: 'denied',
  recordedTimedOut: 'timed out',
  recordedCancelled: 'cancelled',
  noResultRecorded: 'No result recorded',
  statusUnknown: 'Status unknown',
  sectionArguments: 'Arguments',
  sectionOutput: 'Output',
  sectionError: 'Error',
  truncatedAt: (n: number) => `Truncated. Showing the first ${n} characters.`,
  copyFullValue: 'Copy full value',
  contentNotCaptured: 'Content not captured',
  noOutput: 'No output',
  inferredTag: 'inferred',
  inferredTip: 'Matched to this turn by timing',
  linkUnknown: 'Link unknown',
  toolCalls: 'Tool calls',
  // The Waiting queue (v1c §5.9, DS9, DP3).
  viewWaiting: 'Waiting',
  viewWaitingCount: (n: string) => `Waiting (${n})`,
  noWaiting: 'No waiting requests in your loaded chats.',
  noWaitingMatch: (q: string) => `No waiting chats match '${q}'.`,
  noWaitingHint: 'When an agent asks you to approve or answer something, the chat shows here.',
  outsideChat: (n: number) =>
    n === 1
      ? "1 request couldn't be linked to a chat."
      : `${n} requests couldn't be linked to a chat.`,
  superuserDropped: "Requests from other people's chats aren't shown here.",
  lastChecked: (ago: string) => `Last checked ${ago}`,
  requestCount: (n: number) => `${n} requests`,
  requestGone: 'That request was already answered or has expired.',
  // Background-turn signals (v1c §5.8).
  replyReadyIn: 'Reply ready in',
  dismissReplyReady: 'Dismiss',
  untitledChat: 'Untitled chat',
  replyFailedIn: (title: string) => `Reply failed in ${title}`,
  announceReplyReady: (title: string) => `Reply ready in ${title}`,
  rowWaiting: 'waiting for you',
  rowInProgress: 'reply in progress',
  rowFailed: 'reply failed',
  rowNewReply: 'new reply',
  chatsTrigger: (waiting: number, newReplies: number) =>
    [
      'Chats',
      waiting ? `${waiting} waiting` : '',
      newReplies ? `${newReplies} new ${newReplies === 1 ? 'reply' : 'replies'}` : '',
    ]
      .filter(Boolean)
      .join(', '),
  metadataOnly:
    'Only metadata was recorded for this session. Turn on content capture in the harness integration to see messages.',
  tryIt: 'Try it',
  sendAnyway: 'Send anyway',
  debugFrames: 'Raw frames (debug)',
  debugSignals: (seen: string | null, completions: string) =>
    `signals · seen ${seen ?? 'never'} · ${completions || 'no completions'}`,
  debugWaiting: (source: string | null, requests: string) =>
    source ? `waiting · ${source} · ${requests}` : 'waiting · none',
  routedMetric: (m: { total: number; zero: number; one: number; twoPlus: number; empty: number }) =>
    `routed metric · ${m.total} ${m.total === 1 ? 'turn' : 'turns'} · 0 agents ${m.zero} · 1 agent ${m.one} · 2+ agents ${m.twoPlus} · empty ${m.empty}`,
  stopReceiving: 'Stop receiving',
  goToRequest: 'Go to request',
  startNewChat: 'Start a new chat',
  replaceDraft: 'Replace your draft?',
  phaseStarting: 'Starting chat…',
  phaseWaiting: (agent: string) => `Waiting for ${agent}…`,
  phaseStillWorking:
    'Still working. You can leave this page; the reply keeps coming while this tab is open.',
  elapsed: (s: number) => `${s}s`,
  notSent: 'Not sent',
  tryAgain: 'Try again',
  saveAgain: 'Save again',
  discardReply: 'Discard reply',
  details: 'Details',
  serverSaid: (t: string) => `Server said: ${t}`,
  viewTrace: 'View trace',
  traceTitle: 'Trace for this reply',
  openFullTrace: 'Open full trace',
  traceWaiting: 'Looking for the trace. A fresh reply can take a few seconds to reach Tempo.',
  traceStillArriving: 'Spans may still be arriving.',
  traceNotInYet: 'Trace not in yet',
  traceNotInYetFix:
    'Tempo has no spans for this reply yet. Retry in a minute, or open the full trace from Sessions later.',
  traceForbidden: "You can't see this trace.",
  agentLogs: 'Agent logs',
  copyReply: 'Copy',
  copied: 'Copied',
  download: 'Download',
  copyCode: 'Copy code',
  imageLink: (alt: string) => (alt ? `Image: ${alt}` : 'Image'),
  tools: (n: number) => (n === 1 ? '1 tool' : `${n} tools`),
  steps: 'Steps',
  stepRunning: 'running',
  stepFailed: 'failed',
  stepDone: 'done',
  stepNoResult: 'no result',
  copyFailed: "Couldn't copy",
  editAndSend: 'Edit and send',
  openAgent: 'Open the agent',
  agentsPage: 'Agents',
  statusRunning: 'running',
  statusNotRunning: 'not running',
  lockedForbidden: "You can't use this agent. Ask its owner for access.",
  theAgent: 'the agent',
  examples: 'Examples',
  // The apps an agent can use (components/AgentApps.tsx): each runs on the viewer's own account.
  worksWith: 'Works with',
  worksWithHint:
    'Try it now, or connect your apps so it can work with your own content, on your own account.',
  worksWithHintConnected: 'Your apps are connected. Ask it to use them, or just send your text.',
  appConnected: (app: string) => `${app}: connected`,
  moreApps: (n: number) => `+${n} more`,
  moreAppsLabel: (n: number) => `${n} more apps: see all toolkits on MCP servers`,
  replyConnect: 'Connect it so the agent can use it here:',
  loading: 'Loading…',
  usageLoaded: 'Usage for loaded messages',
  turnsWithoutUsage: (n: number) => (n === 1 ? '1 turn without usage' : `${n} turns without usage`),
  costUnknown: 'cost unknown',
  tokens: (n: string) => `${n} tokens`,
  announceReply: (agent: string) => `Reply from ${agent} complete`,
  announceRequest: (agent: string) => `Approval requested by ${agent}`,
  announceFailed: 'The turn failed',
  request: {
    header: 'The agent needs you',
    requestedBy: 'Requested by',
    somethingElse: 'Something else',
    submit: 'Submit',
    approveOnce: 'Approve once',
    alwaysAllow: (tool: string) => `Always allow ${tool} in this chat`,
    alwaysAllowHint: "Future calls of this tool in this chat won't ask again.",
    reject: 'Tell the agent no',
    dismiss: 'Dismiss request',
    dismissHint: "Ends the request without an answer; the agent won't continue this step.",
    noArguments: "This server doesn't send the tool's arguments.",
    signIn: (provider: string) => `Sign in to ${provider}`,
    opensHost: (host: string) => `Opens ${host}`,
    waitingSignIn: 'Waiting for sign-in…',
    signedIn: "I've signed in",
    pager: (i: number, n: number) => `Request ${i} of ${n}`,
    pagerLabel: 'Pending requests',
    previous: 'Previous',
    next: 'Next',
    thisTool: 'this tool',
    theService: 'the service',
    handledElsewhere: 'Handled elsewhere.',
    receipt: {
      resolved: 'Answered',
      rejected: 'Declined',
      canceled: 'Request canceled',
      expired: 'Request expired',
      pending: 'Waiting',
    } as Record<string, string>,
    noCancel: (date: string) =>
      `This chat is paused until you answer or the request expires on ${date}.`,
    submitFailed: "Couldn't send your answer.",
  },
} as const
