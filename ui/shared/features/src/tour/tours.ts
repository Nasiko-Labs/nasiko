/**
 * Every tour's steps. A step with a `target` points at `[data-tour~="<target>"]` and is shown only while that element
 * is on screen when the tour opens, so one tour serves a page's states (empty, member, phone) by dropping the steps
 * that don't apply. A step without one is a centred card: each tour opens with one, so it never comes out empty.
 *
 * Written for someone on the page for the first time: each step says what they can do there, not only what the area
 * is. Name only what the page really has (labels, tabs, keys), in every edition: a layer's own items are its tours'.
 *
 * Shared targets: `page-actions` and `page-stepper` are the shell top bar's slots (TopBar.tsx), `tour-button` its
 * help button (`TourButton`), `section-nav` the shared sub-nav (SectionNav). `tours.test.tsx` renders each page and
 * checks its targets still exist.
 */
import type { TourStep } from '@nasiko/app-core/tour'
import type { TourId } from './logic'

export const TOURS: Record<TourId, readonly TourStep[]> = {
  // Compact (user 2026-10-22): one line per group, opened from the account menu's Tour the sidebar. Titles are the
  // groups' own headings (`shell/copy.ts` `nav`); the first group has none.
  app: [
    {
      title: 'The sidebar',
      body: 'Every page, grouped by what you do there. ⌘B (Ctrl+B) folds it to icons.',
    },
    {
      target: 'app-menu',
      title: 'Every page is in the menu',
      body: 'Open it to move between pages.',
    },
    {
      target: 'nav-work',
      side: 'right',
      title: 'Home and Chat',
      body: 'Home is your fleet at a glance. Chat talks to one agent, or to the Orchestrator, which picks one for you.',
    },
    {
      target: 'nav-fleet',
      side: 'right',
      title: 'Fleet',
      body: 'Your agents, coding harnesses and workflows: deploy, start and stop agents, and chain them into workflows.',
    },
    {
      target: 'nav-observe',
      side: 'right',
      title: 'Observe',
      body: 'Sessions and flows: what each request did, step by step, and what it cost.',
    },
    {
      target: 'nav-tokenops',
      side: 'right',
      title: 'TokenOps',
      body: 'What your agents cost, what drives it, and what optimization saves.',
    },
    {
      target: 'nav-connect',
      side: 'right',
      title: 'Connect',
      body: 'The models (LLM router) and tools (MCP servers) your agents can call.',
    },
    {
      target: 'nav-views',
      side: 'right',
      title: 'Views',
      body: 'Views you saved, ready to open again.',
    },
    {
      target: 'nav-org',
      side: 'right',
      title: 'Organization',
      body: 'Settings for the whole workspace, and the secrets your agents use.',
    },
  ],

  // Every section the Overview has (redesign 2026-10-19, the village and map 2026-10-16, the first run 2026-10-21);
  // each step shows only where its section is on screen, so one tour serves the cards, the canvas and the first run.
  overview: [
    {
      title: 'Home',
      body: 'Your fleet at a glance: what it costs, what needs you and how healthy each agent is. Each card links to the page that owns it.',
    },
    {
      target: 'overview-views',
      title: 'Pick a view',
      body: 'Overview is the cards. The other views draw the same fleet as a place you can explore. Your choice is remembered.',
    },
    {
      // Only the summary, Spend, harness and savings cards take `range` (OverviewPage.tsx).
      target: 'overview-range',
      title: 'Pick the window',
      body: 'Switch between 7, 30 and 90 days. The summary, Spend, harness and savings cards follow it; the others keep their own windows.',
    },
    {
      target: 'overview-summary',
      title: 'The summary',
      body: 'Fleet spend in one sentence, then tokens, agent runs, coding harnesses and what token optimization saved.',
    },
    {
      target: 'overview-canvas',
      title: 'Your estate',
      body: 'Agents, harnesses and spend, laid out as one place. Select anything to see its details and what to do about it.',
    },
    {
      target: 'overview-try-run',
      title: 'Try your agent',
      body: 'Give this agent a real task here, then run the same task with Token optimization on and compare what it costs.',
    },
    {
      target: 'overview-setup-guide',
      title: 'Get your first agent running',
      body: 'Add your model key and deploy a first agent. Each step opens the setup guide where you left off.',
    },
    {
      target: 'overview-cli',
      title: 'Or use the CLI',
      body: 'The same steps as three commands, ready to copy.',
    },
    {
      target: 'overview-needs',
      title: 'Needs your attention',
      body: 'Agents that need action, requests waiting for your answer and failed sessions, worst first, then what is left to set up.',
    },
    {
      target: 'overview-health',
      title: 'Fleet health',
      body: 'Each agent rated Healthy, Watch or Needs action. Click a count to see those agents.',
    },
    {
      // The chart itself isn't clickable: only the spike line links to sessions (Spend.tsx).
      target: 'overview-spend',
      title: 'Spend',
      body: 'Spend per day, stacked by the agents behind it, against the period before. A spike links to the sessions that caused it.',
    },
    {
      target: 'overview-harness-activity',
      title: 'Coding harness activity',
      body: 'What your coding harnesses did and cost in the window, by model. Its link opens Coding harnesses.',
    },
    {
      target: 'overview-sessions',
      title: 'Recent sessions',
      body: 'The latest runs with their status and cost. Open one to follow it step by step.',
    },
    {
      target: 'overview-saved',
      title: 'Saved tokens by agent',
      body: 'What token optimization saved for each agent, largest first.',
    },
    {
      target: 'overview-chats',
      title: 'Recent chats',
      body: 'Pick a conversation up where you left it.',
    },
    {
      // The empty fleet's panel (SetUpNow.tsx); the summary has no Quick actions since 2026-11-06.
      target: 'overview-actions',
      title: 'Works before your first agent',
      body: 'Ask the Orchestrator, add your model key, connect an MCP server or a coding harness. None of it needs an agent yet.',
    },
  ],

  chat: [
    {
      title: 'Chat',
      body: 'Talk to one agent directly, or ask the Orchestrator and let it pick the right agent for each question.',
    },
    {
      target: 'chat-rail',
      side: 'right',
      title: 'Your chats',
      body: 'Search them or start a New chat. Chats is your history by day, Waiting holds requests agents need you to answer, and Recorded your coding harness sessions.',
    },
    {
      target: 'chat-target',
      title: 'Who gets the message',
      body: 'Pick an agent, or the Orchestrator to route each question for you.',
    },
    {
      target: 'chat-composer',
      side: 'top',
      title: 'Write here',
      body: 'Enter sends, Shift+Enter adds a line. Attach files to send them with the message.',
    },
    {
      // An open chat's header (SessionHeader.tsx); New chat has none, so the step drops there.
      target: 'page-actions',
      title: 'This chat',
      body: 'View session opens its full trace. The ⋯ menu shows what it used, and renames, exports or deletes the chat.',
    },
    {
      target: 'page-stepper',
      title: 'Step through chats',
      body: 'Move to the previous or next chat with these arrows, or J and K.',
    },
  ],

  agents: [
    {
      title: 'Agents',
      body: 'Every agent you can use, and the ones you run. Open one to chat with it, see what it costs or change it.',
    },
    {
      target: 'section-nav',
      title: 'Three views',
      body: 'All agents, Your agents (the ones you run) and Builds (every deploy and how it went).',
    },
    {
      target: 'page-actions',
      title: 'Deploy an agent',
      body: 'Upload a zip, build from a GitHub repository or import from a registry.',
    },
    {
      target: 'agents-empty',
      title: 'No agents yet',
      body: 'Deploy your first agent from a zip, a GitHub repository or a registry. It shows up here once it runs.',
    },
    {
      target: 'agents-toolbar',
      title: 'Find an agent',
      body: 'Search by name, description, tag or skill (press /), filter by owner or tag, and switch between a grid and a list.',
    },
    {
      target: 'agents-grid',
      side: 'top',
      title: 'The catalog',
      body: 'Each agent with its status, what it does and whether it is yours. Open one for its details and activity.',
    },
    {
      target: 'mine-needs',
      title: 'Needs attention',
      body: 'Your agents that crashed or failed, with their logs one click away.',
    },
    {
      target: 'mine-status-tabs',
      title: 'Filter by status',
      body: 'All, Running, Parked, Deploying, Needs attention, Stopped and Coding harnesses, each with its count. A parked agent sat idle, so its container was released; the next message starts it again.',
    },
    {
      target: 'mine-table',
      side: 'top',
      title: 'Run your agents',
      body: "Each row has its version, 24-hour turns and cost. The ⋯ menu starts, stops or restarts it and shows its logs; roll back from the agent's Versions tab.",
    },
  ],

  agent: [
    {
      title: 'An agent',
      body: 'What this agent does, how it is running, and everything you can change about it.',
    },
    {
      target: 'agent-header',
      title: 'At a glance',
      body: 'Name, status, id, version, owner and who can see it.',
    },
    {
      target: 'page-actions',
      title: 'Run it',
      body: 'Try it in chat, start, stop or restart it, and copy its CLI commands from the ⋯ menu.',
    },
    {
      target: 'agent-tabs',
      title: 'Tabs',
      body: 'Overview says what it does and how it is doing; Activity, Versions (where you roll back), MCP and Details follow. Whoever manages it also gets Builds, Access and Settings.',
    },
    {
      // The Overview tab (HubTimeline.tsx), where the page opens.
      target: 'agent-overview',
      side: 'top',
      title: 'How it is doing',
      body: 'What needs you first, then its spend, requests and latency over the window you pick. Click a bar for that slice, or Compare with the window before.',
    },
    {
      // Only on the Details tab (DetailsTab.tsx).
      target: 'agent-facts',
      side: 'left',
      title: 'How it is wired',
      body: 'The model and key it routes to, the image it runs, and its A2A details.',
    },
    {
      target: 'page-stepper',
      title: 'Next agent',
      body: 'Step through the list you came from with these arrows, or J and K.',
    },
  ],

  deploy: [
    {
      title: 'Deploy an agent',
      body: 'Bring an agent from a zip, a GitHub repository or a registry. Nasiko builds it and starts it.',
    },
    {
      target: 'deploy-methods',
      title: 'Choose a source',
      body: 'Upload a .zip, pick a GitHub repository to clone and build, or import a published image.',
    },
    {
      target: 'deploy-upload-form',
      title: 'Upload',
      body: 'Drop the zip, check the name and version, then Deploy. The file is checked here before it is sent.',
    },
    {
      target: 'deploy-requirements',
      side: 'left',
      title: 'What the zip needs',
      body: 'A Dockerfile at the root, an entrypoint and a version. This list ticks off as your file passes.',
    },
    {
      target: 'deploy-cli',
      side: 'left',
      title: 'Prefer the terminal?',
      body: 'Copy the nasiko upload command and deploy from your shell instead.',
    },
    {
      target: 'deploy-github',
      title: 'From GitHub',
      body: 'Connect GitHub, pick a repository, check the branch and name, then Deploy. Nasiko clones it and builds it.',
    },
    {
      target: 'deploy-registry',
      title: 'From a registry',
      body: 'Paste an image reference (host/owner/name:tag), then Import and deploy. It can take a few minutes.',
    },
  ],

  builds: [
    {
      title: 'Builds',
      body: 'Every deploy and how it went, newest first.',
    },
    {
      target: 'page-actions',
      title: 'Deploy an agent',
      body: 'Start a new deploy from a zip, GitHub or a registry.',
    },
    {
      target: 'builds-filters',
      title: 'Filter builds',
      body: 'Show one status at a time, or search by agent.',
    },
    {
      // The Build page has no logs yet ("Build logs need a newer Open Runtime server").
      target: 'builds-panel',
      side: 'top',
      title: 'Build history',
      body: 'Agent, version, status, source and duration. A failed build says why; open one for its stages and how to fix it.',
    },
  ],

  workflows: [
    {
      title: 'Workflows',
      body: 'Chain several agents into one job: each step hands its result to the next.',
    },
    {
      target: 'page-actions',
      title: 'Create a workflow',
      body: 'Describe what you want and Nasiko drafts the steps, or add them yourself.',
    },
    {
      // Runs reads one page of 50 (workflows/tuning.ts RUNS_LIMIT).
      target: 'section-nav',
      title: 'Deployed, drafts and runs',
      body: 'Deployed workflows are ready to run, drafts are still being shaped, and Runs lists the 50 most recent runs.',
    },
    {
      // Sort shows only where the server reports run metrics, so it isn't promised.
      target: 'workflows-toolbar',
      title: 'Find a workflow',
      body: 'Search by name or description.',
    },
    {
      target: 'workflows-list',
      side: 'top',
      title: 'Your workflows',
      body: 'Steps, runs, health and the last run. The ⋮ menu runs or deletes a workflow.',
    },
    {
      target: 'runs-filters',
      title: 'Filter runs',
      body: 'Search runs, and filter by status or by when they started.',
    },
    {
      target: 'runs-list',
      side: 'top',
      title: 'Every run',
      body: 'Expand a run for its steps and tokens, answer a paused step right here, or View trace. Failed or stopped runs can be run again.',
    },
  ],

  router: [
    {
      title: 'LLM router',
      body: 'Which model and key each agent uses. This is the routing you configured, not a log of calls.',
    },
    {
      target: 'router-how',
      title: 'How routing works',
      body: 'Opens a five-step explainer of how a call finds its model.',
    },
    {
      target: 'router-tabs',
      title: 'Tabs',
      body: 'Agents shows what each agent uses. A config is a saved model, provider and key: add one with New config and set one as the default. Providers lists where calls can go.',
    },
    {
      target: 'router-agent-filters',
      title: 'Filter agents',
      body: 'By routing source (default, attached or none), with counts. A large fleet also gets a search by name.',
    },
    {
      target: 'router-agents',
      side: 'top',
      title: 'Your agents',
      body: 'The model, key and config each one uses, and its spend. Change routing from the row.',
    },
  ],

  mcp: [
    {
      title: 'MCP servers',
      body: 'Tools your agents can call: servers you add, servers shared with you, and platform toolkits.',
    },
    {
      target: 'page-actions',
      title: 'Add a server',
      body: 'Register a remote MCP server, or upload your own and let Nasiko build it.',
    },
    {
      target: 'mcp-toolbar',
      title: 'Narrow the list',
      body: 'Show what you can connect or have connected, filter by source, and search (press /).',
    },
    {
      target: 'mcp-grid',
      side: 'top',
      title: 'Connect',
      body: 'Each card shows its tools and owner. Connect signs you in or asks for your key. Open a server you added, or one shared with you, to choose which agents may use it.',
    },
  ],

  sessions: [
    {
      title: 'Sessions',
      body: 'Every conversation with your agents: what was asked, what it cost, and how it went.',
    },
    {
      target: 'page-actions',
      title: 'Pick the window',
      body: 'Presets or a custom UTC range; the list follows it. Arriving from a TokenOps day, Clear day goes back to the window.',
    },
    {
      target: 'sessions-toolbar',
      title: 'What needs attention',
      body: 'Jump to failing, slow or costly sessions, filter by agent, or pause the live list.',
    },
    {
      target: 'sessions-list',
      side: 'top',
      title: 'The sessions',
      body: 'Start, agent, first input, duration, cost and status. Expand a row to open its agent, chat or flow, or open the session for its full trace.',
    },
  ],

  session: [
    {
      title: 'A session',
      body: 'What happened in one conversation, from the question to every model and tool call.',
    },
    {
      target: 'session-actions',
      title: 'Go further',
      body: 'Open the chat or the flow it belongs to, see its day in TokenOps, or copy a link to it.',
    },
    {
      target: 'session-narrative',
      title: 'What happened',
      body: 'A plain summary of the trace. Click a phrase to select the span it describes.',
    },
    {
      target: 'session-spans',
      side: 'top',
      title: 'Every step',
      body: 'Each span as a waterfall or a table. Filter spans to find a tool or a model call.',
    },
    {
      target: 'session-span-panel',
      side: 'left',
      title: 'Span details',
      body: 'Inputs, outputs, tokens and timing for the selected span. Its ↑ and ↓ buttons move to the previous or next span.',
    },
    {
      target: 'page-stepper',
      title: 'Next session',
      body: 'Step through the list you came from with these arrows, or J and K.',
    },
  ],

  flows: [
    {
      title: 'Flows',
      body: 'Every request to your agents, and every agent it reached on the way.',
    },
    {
      target: 'page-actions',
      title: 'Pick the window',
      body: 'Presets or a custom UTC range, 7 days to start. The charts and the table follow it.',
    },
    {
      target: 'flows-filters',
      title: 'Narrow it down',
      body: 'Search requests, and filter by kind or status.',
    },
    {
      target: 'flows-charts',
      title: 'Requests and timing',
      body: 'How many requests came in, and how long finished ones took.',
    },
    {
      target: 'flows-table',
      side: 'top',
      title: 'Each request',
      body: 'Question, agent, status and duration. Open one to see every hop on a timeline.',
    },
  ],

  tokenops: [
    {
      title: 'TokenOps',
      body: 'Where your spend goes: by day, by agent or workflow, and against the month so far.',
    },
    {
      target: 'page-actions',
      title: 'Pick the window',
      body: '24 hours to last month, or a custom range. Every panel follows it.',
    },
    {
      target: 'tokenops-filters',
      title: 'Narrow it down',
      body: 'Filter every number by agent, provider or model.',
    },
    {
      target: 'tokenops-summary',
      title: 'The short version',
      body: 'Spend for the window against the period before, and how the month is tracking.',
    },
    {
      target: 'tokenops-sections',
      side: 'top',
      title: 'Dig in',
      body: 'Spend, Drivers and Optimization, all on this page; the bar above jumps between them. Click a day for its sessions. In Drivers, switch between agents and workflows, export a CSV or view traces.',
    },
  ],

  optimization: [
    {
      title: 'Optimization',
      body: 'How Nasiko cuts the tokens each chat sends to the model, what that saves, and where to turn more of it on.',
    },
    {
      target: 'optimization-header',
      title: 'Window and settings',
      body: 'Pick the window for the savings, or jump to your own settings.',
    },
    {
      target: 'optimization-lead',
      title: 'The answer',
      body: 'How much is being saved and what the numbers cover. Click a bar to narrow the biggest senders to it.',
    },
    {
      target: 'optimization-attention',
      title: 'Turn it on',
      body: 'Agents with Token optimization off. Turn it on for all of them at once from here.',
    },
    {
      target: 'optimization-by-agent',
      side: 'top',
      title: 'By agent',
      body: 'What each agent sends and saves, and whether optimization is on for it. Sort by any column, or Show all.',
    },
    {
      target: 'optimization-senders',
      side: 'top',
      title: 'Biggest senders',
      body: 'The largest requests in the window. Open one of your chats to see its trace.',
    },
    {
      target: 'optimization-settings',
      side: 'top',
      title: 'Your settings',
      body: 'How history is chosen for your chats, and how much of it is carried.',
    },
  ],

  harnesses: [
    {
      title: 'Coding harnesses',
      body: 'Your coding harnesses (like Claude Code or Codex): sessions, models and estimated cost.',
    },
    {
      target: 'harnesses-header',
      title: 'Window and compare',
      body: 'Pick the window, and compare it with the period before.',
    },
    {
      target: 'harnesses-summary',
      title: 'Your usage',
      body: 'What you did and what it cost, with daily activity and top models.',
    },
    {
      target: 'harnesses-cards',
      side: 'top',
      title: 'Each harness',
      body: "Sessions and cost per harness. A card's filter button narrows the page to it.",
    },
    {
      target: 'harnesses-sessions',
      side: 'top',
      title: 'Recent sessions',
      body: 'Your latest harness sessions. Open one for its turns, tools and cost.',
    },
    {
      target: 'harnesses-connect',
      side: 'top',
      title: 'Connect a harness',
      body: 'Run these in the Nasiko CLI on the machine where the harness runs. Its sessions show up here after that.',
    },
  ],

  // Every /settings page: the workspace form, Secrets, Optimization tiers, MCP toolkits and any layer's section.
  settings: [
    {
      title: 'Settings',
      body: 'How this workspace runs, and the secrets your agents use. Your own appearance and password are in Account settings, in the account menu.',
    },
    {
      target: 'settings-sections',
      side: 'right',
      title: 'Sections',
      body: 'Workspace sections first, then Security with Secrets. Everyone has Secrets; the workspace sections are for admins.',
    },
    {
      // MCP toolkits, Optimization tiers and a layer's sections each save on their own (SettingsPage.tsx).
      target: 'settings-form',
      side: 'top',
      title: 'One Save',
      body: 'General, Flow limits and Registry are one form: change any of them, then Save once. Other sections save on their own.',
    },
    {
      target: 'settings-mcp',
      side: 'top',
      title: 'MCP toolkits',
      body: 'Your Composio key, and which toolkits people can connect. Unticking one hides it; nothing is deleted.',
    },
    {
      target: 'settings-tiers',
      side: 'top',
      title: 'Optimization tiers',
      body: "What Low, Medium and High mean for everyone's chats. People pick their tier on the Optimization page.",
    },
    {
      target: 'secrets-list',
      side: 'top',
      title: 'Your secrets',
      body: 'API keys, stored encrypted and masked. Show and Copy read a value only when you click; Delete names the router configs that use it.',
    },
    {
      target: 'secrets-add',
      side: 'top',
      title: 'Add a secret',
      body: 'A name and a value. Every agent you own gets it as an environment variable (restart the agent to apply a change), and router configs can use it as a key.',
    },
  ],

  build: [
    {
      title: 'A build',
      body: 'One deploy, from the upload or clone to a running agent.',
    },
    {
      // In the top bar only while it builds or after it failed; a running agent's links are in the outcome card.
      target: 'page-actions',
      title: 'Open the agent',
      body: 'Jump to the agent while it builds, or after it failed.',
    },
    {
      target: 'build-outcome',
      title: 'How it went',
      body: 'Running, built but not running, or failed. A running agent can be chatted with or opened from here; a failed build says what went wrong and how to deploy again.',
    },
    {
      target: 'build-stages',
      side: 'top',
      title: 'Each stage',
      body: 'Queued, building the image, deploying, running. This page updates live, and the build keeps going if you leave.',
    },
  ],

  // One workflow (deployed or draft) and one of its runs (`?run=`): each step shows only where its part is drawn.
  workflow: [
    {
      title: 'A workflow',
      body: 'Its steps and which agent runs each one. Run it or edit it here; a run shows what each step did.',
    },
    {
      target: 'wf-plan',
      title: 'Describe it instead',
      body: 'Say what the workflow should do and Generate plan drafts the steps for you.',
    },
    {
      target: 'wf-steps',
      side: 'top',
      title: 'The steps',
      body: "Each step is a task for one agent, run in order. Each one gets the previous step's result.",
    },
    {
      target: 'wf-run',
      side: 'top',
      title: 'Run or edit',
      body: 'Run starts it now and opens the run. Edit changes its steps.',
    },
    {
      target: 'wf-save',
      side: 'top',
      title: 'Save and run',
      body: 'Save & run saves your changes and starts a run. A draft also has Deploy, which makes it ready to run from the list.',
    },
    {
      target: 'wf-run-steps',
      side: 'top',
      title: 'Each step',
      body: 'What each agent did, with its tokens. Open flow follows a step through every agent, and a paused step asks for your answer here.',
    },
    {
      target: 'wf-run-output',
      side: 'top',
      title: 'The result',
      body: "The workflow's final answer.",
    },
    {
      target: 'wf-danger',
      side: 'top',
      title: 'Delete',
      body: 'Removes the workflow. Runs that already happened are not affected.',
    },
    {
      target: 'page-stepper',
      title: 'Next workflow',
      body: 'Step through the list you came from with these arrows, or J and K.',
    },
  ],

  'workflow-new': [
    {
      title: 'New workflow',
      body: 'Chain agents into one job. Describe it and let Nasiko draft the steps, or write them yourself.',
    },
    {
      target: 'wf-name',
      title: 'Name it',
      body: 'A name your team will recognize in the list and in runs.',
    },
    {
      target: 'wf-plan',
      title: 'Describe it',
      body: 'Say what should happen, then Generate plan. You can edit every step it drafts.',
    },
    {
      target: 'wf-steps',
      side: 'top',
      title: 'The steps',
      body: 'Add, reorder (drag the grip, or use the arrow keys on it) and remove steps, each with its task and agent.',
    },
    {
      target: 'wf-save',
      side: 'top',
      title: 'Save',
      body: 'Save it as a draft to test first, or deploy it ready to run.',
    },
  ],

  'mcp-server': [
    {
      title: 'An MCP server',
      body: 'One server: its tools, your connection to it, and which agents may use it.',
    },
    {
      target: 'mcp-detail-actions',
      side: 'left',
      title: 'Connect',
      body: 'Connect or disconnect your account. The ⋯ menu copies its id or link, and deletes the server if it is yours.',
    },
    {
      target: 'mcp-detail-tabs',
      title: 'Tabs',
      body: 'Agents is where you choose which agents may call its tools, and whether each tool is allowed, asks first or is blocked. Its owner also gets Access to share it, Settings, and Logs for an uploaded server.',
    },
    {
      target: 'mcp-detail-overview',
      side: 'top',
      title: 'Its tools',
      body: 'Details, your connection, and every tool this server offers.',
    },
  ],

  flow: [
    {
      title: 'A flow',
      body: 'One request, followed through every agent it reached.',
    },
    {
      target: 'flow-narrative',
      title: 'What happened',
      body: 'The flow in plain words. When it is waiting on you, Answer the request is right below.',
    },
    {
      target: 'flow-kpis',
      title: 'The numbers',
      body: 'Status, duration and when it started, plus steps, agents and any wait for a human.',
    },
    {
      target: 'flow-view',
      title: 'Timeline or table',
      body: 'The timeline draws each call by agent over time; the table lists them. Critical path marks the calls that set the total time.',
    },
    {
      target: 'flow-steps',
      side: 'top',
      title: 'Steps',
      body: 'Every agent call in the order it started: its input, output, timing and what went wrong. Select one on the timeline to jump to it.',
    },
    {
      target: 'page-stepper',
      title: 'Next flow',
      body: 'Step through the list you came from with these arrows, or J and K.',
    },
  ],

  account: [
    {
      title: 'Account settings',
      body: 'Yours alone: how Nasiko looks in this browser, your password, and the keys Nasiko answers.',
    },
    {
      target: 'settings-sections',
      side: 'right',
      title: 'Sections',
      body: 'Appearance, Password and Keyboard shortcuts.',
    },
    {
      target: 'appearance-mode',
      title: 'Mode',
      body: 'System, Light or Dark, for this browser.',
    },
    {
      target: 'appearance-theme',
      side: 'top',
      title: 'Theme',
      body: 'Carbon, Teal, Indigo or Plum. The account menu has a quick switch too.',
    },
    {
      target: 'password-form',
      side: 'top',
      title: 'Change your password',
      body: 'Your current password, then the new one twice. Changing it signs you out.',
    },
    {
      target: 'shortcuts-list',
      side: 'top',
      title: 'Keyboard shortcuts',
      body: 'Every key Nasiko answers, by where it works. Press ? on any page for its tour.',
    },
  ],

  status: [
    {
      title: 'Status',
      body: 'Whether this page can reach the Nasiko server.',
    },
    {
      target: 'status-backend',
      title: 'The server',
      body: 'Connected, checking or unreachable. When it is unreachable, this says how to start it.',
    },
  ],
}
