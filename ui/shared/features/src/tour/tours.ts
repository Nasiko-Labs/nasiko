// Ported from the legacy ui/ on development (3aac84f66) common/src/features/tour/tours.ts (dd9fe0f64). Local edits:
// `TourStep` from @nasiko/app-core/tour; Prettier formatting; the app tour follows this sidebar (no TokenOps group);
// the overview tour is development's for the Overview page (no Home rows or scope).
/**
 * Every tour's steps. A step with a `target` points at `[data-tour~="<target>"]` and is shown only while that element
 * is on screen when the tour opens, so one tour serves a page's states (empty, member, phone) by dropping the steps
 * that don't apply. A step without one is a centred card: each tour opens with one, so it never comes out empty.
 *
 * Shared targets: `page-actions` and `page-stepper` are the shell top bar's slots (TopBar.tsx), `tour-button` its
 * help button (`TourButton`), `section-nav` the shared sub-nav (SectionNav). `tours.test.tsx` renders each page and
 * checks its targets still exist.
 */
import type { TourStep } from "@nasiko/app-core/tour";
import type { TourId } from "./logic";

/**
 * A page's steps, with the app tour folded in when `withApp`: its welcome and the sidebar first, then the page, and
 * the tour button last, so the tour ends on where to find the next one.
 */
export function stepsFor(page: readonly TourStep[], withApp: boolean): readonly TourStep[] {
  const app = TOURS.app;
  if (!withApp || page === app) return page;
  const last = app[app.length - 1];
  return [...app.slice(0, -1), ...page, ...(last ? [last] : [])];
}

export const TOURS: Record<TourId, readonly TourStep[]> = {
  app: [
    {
      title: "Welcome to Nasiko",
      body: "A quick look around: where your agents live, how they connect to models and tools, and where to watch what they do and spend.",
    },
    {
      target: "app-menu",
      title: "Every page is in the menu",
      body: "Open it to move between Chat, Agents, Observe and Settings.",
    },
    {
      target: "nav-work",
      side: "right",
      title: "Start here",
      body: "Overview shows what needs you today. Chat talks to one agent, or lets the Orchestrator pick the right one.",
    },
    {
      target: "nav-fleet",
      side: "right",
      title: "Your fleet",
      body: "Agents are what you deploy and run. Workflows chain several agents into one repeatable job.",
    },
    {
      target: "nav-connect",
      side: "right",
      title: "What agents can call",
      body: "The LLM router decides which model and key each agent uses. MCP servers give agents tools.",
    },
    {
      target: "nav-observe",
      side: "right",
      title: "Watch and pay",
      body: "Sessions and Flows show what happened, step by step. TokenOps, Optimization and Harnesses show what it cost and how to spend less.",
    },
    {
      target: "nav-org",
      side: "right",
      title: "Workspace settings",
      body: "Defaults for the router, flow limits, the registry and your secrets.",
    },
    {
      target: "tour-button",
      title: "Lost? Take a tour",
      body: "Every page has its own short tour. Start it from this button, or press ?.",
    },
  ],

  overview: [
    {
      title: "Home",
      body: "Your fleet at a glance: what needs you, what it costs, and how healthy each agent is.",
    },
    {
      target: "overview-range",
      title: "Pick the window",
      body: "Switch between 7, 30 and 90 days. The headline, Spend and Harnesses follow it.",
    },
    {
      target: "overview-summary",
      title: "The headline",
      body: "Fleet spend, runs and agents in one sentence, compared with the period before.",
    },
    {
      target: "overview-needs",
      title: "Needs you",
      body: "Agents that need action, requests waiting for your answer and failed sessions, worst first.",
    },
    {
      target: "overview-health",
      title: "Fleet health",
      body: "Each agent rated Healthy, Watch or Needs action. Click a count to see those agents.",
    },
    {
      target: "overview-setup-guide",
      title: "Finish setting up",
      body: "Pick your role, connect a model and deploy a first agent. Each step opens the setup guide where you left off.",
    },
    {
      target: "overview-actions",
      title: "Quick actions",
      body: "Start a chat, ask the Orchestrator or deploy an agent from here.",
    },
  ],

  chat: [
    {
      title: "Chat",
      body: "Talk to one agent directly, or ask the Orchestrator and let it pick the right agent for each question.",
    },
    {
      target: "chat-rail",
      side: "right",
      title: "Your chats",
      body: "History grouped by day. Waiting lists the requests agents are holding for your answer.",
    },
    {
      target: "chat-target",
      title: "Who gets the message",
      body: "Pick an agent, or the Orchestrator to route each question for you.",
    },
    {
      target: "chat-composer",
      side: "top",
      title: "Write here",
      body: "Enter sends, Shift+Enter adds a line.",
    },
    {
      target: "page-stepper",
      title: "Step through chats",
      body: "Move to the previous or next chat with these arrows, or J and K.",
    },
  ],

  agents: [
    {
      title: "Agents",
      body: "Every agent you can use, and the ones you own and run.",
    },
    {
      target: "section-nav",
      title: "Three views",
      body: "All agents, Your agents (the ones you run) and Builds (every deploy and how it went).",
    },
    {
      target: "page-actions",
      title: "Deploy an agent",
      body: "Upload a zip, build from GitHub or import from a registry.",
    },
    {
      target: "agents-toolbar",
      title: "Find an agent",
      body: "Search by name, description, tag or skill, and filter by owner or tag.",
    },
    {
      target: "agents-grid",
      side: "top",
      title: "The catalog",
      body: "Each card shows the status, what it does and who owns it. Open one for its details and activity.",
    },
    {
      target: "mine-status-tabs",
      title: "Filter by status",
      body: "Running, stopped or needing attention, with counts.",
    },
    {
      target: "mine-table",
      side: "top",
      title: "Run your agents",
      body: "Each row has its version, 24-hour turns and cost. The ⋯ menu starts, stops, restarts or rolls back the agent.",
    },
  ],

  agent: [
    {
      title: "An agent",
      body: "What this agent does, how it is running, and everything you can change about it.",
    },
    {
      target: "agent-header",
      title: "At a glance",
      body: "Name, status, id, version, owner and who can see it.",
    },
    {
      target: "page-actions",
      title: "Run it",
      body: "Try it in chat, start, stop or restart it, and copy its CLI commands from the ⋯ menu.",
    },
    {
      target: "agent-tabs",
      title: "Everything else",
      body: "Activity, versions, builds, MCP tools, access and settings each have a tab.",
    },
    {
      target: "agent-facts",
      side: "left",
      title: "How it is wired",
      body: "The model and key it routes to, the image it runs, and its A2A details.",
    },
    {
      target: "page-stepper",
      title: "Next agent",
      body: "Step through the list you came from with these arrows, or J and K.",
    },
  ],

  deploy: [
    {
      title: "Deploy an agent",
      body: "Bring an agent from a zip, a GitHub repository or a registry. Nasiko builds it and starts it.",
    },
    {
      target: "deploy-methods",
      title: "Choose a source",
      body: "Upload a .zip, pick a GitHub repository to clone and build, or import a published image.",
    },
    {
      target: "deploy-upload-form",
      title: "Upload",
      body: "Drop the zip, check the name and version, then Deploy. The file is checked here before it is sent.",
    },
    {
      target: "deploy-requirements",
      side: "left",
      title: "What the zip needs",
      body: "A Dockerfile at the root, an entrypoint and a version. This list ticks off as your file passes.",
    },
    {
      target: "deploy-cli",
      side: "left",
      title: "Prefer the terminal?",
      body: "Copy the nasiko upload command and deploy from your shell instead.",
    },
  ],

  builds: [
    {
      title: "Builds",
      body: "Every deploy and how it went, newest first.",
    },
    {
      target: "builds-filters",
      title: "Filter builds",
      body: "Show one status at a time, or search by agent.",
    },
    {
      target: "builds-panel",
      side: "top",
      title: "Build history",
      body: "Agent, version, status, source and duration. A failed build says why; open one for its stages and logs.",
    },
  ],

  workflows: [
    {
      title: "Workflows",
      body: "Chain several agents into one job: each step hands its result to the next.",
    },
    {
      target: "page-actions",
      title: "Create a workflow",
      body: "Describe what you want and Nasiko drafts the steps, or add them yourself.",
    },
    {
      target: "section-nav",
      title: "Deployed, drafts and runs",
      body: "Deployed workflows are ready to run, drafts are still being shaped, and Runs is every run so far.",
    },
    {
      target: "workflows-toolbar",
      title: "Find a workflow",
      body: "Search by name or description, and sort.",
    },
    {
      target: "workflows-list",
      side: "top",
      title: "Your workflows",
      body: "Steps, runs, health and the last run. The ⋮ menu runs or deletes a workflow.",
    },
    {
      target: "runs-filters",
      title: "Filter runs",
      body: "Search runs, or show the ones that need attention, are running, completed or failed.",
    },
    {
      target: "runs-list",
      side: "top",
      title: "Every run",
      body: "Open a run for its steps and tokens. Failed runs can be run again.",
    },
  ],

  router: [
    {
      title: "LLM router",
      body: "Which model and key each agent uses. This is the routing you configured, not a log of calls.",
    },
    {
      target: "router-how",
      title: "How routing works",
      body: "A five-step explainer of how a call finds its model.",
    },
    {
      target: "router-tabs",
      title: "Agents, configs and providers",
      body: "A config is a saved model, provider and key. One of them is the default.",
    },
    {
      target: "router-agent-filters",
      title: "Filter agents",
      body: "By routing source (default, attached or none), with counts, or by name.",
    },
    {
      target: "router-agents",
      side: "top",
      title: "Your agents",
      body: "The model, key and config each one uses, and its spend. Change routing from the row.",
    },
  ],

  mcp: [
    {
      title: "MCP servers",
      body: "Tools your agents can call: servers you add, servers shared with you, and platform toolkits.",
    },
    {
      target: "page-actions",
      title: "Add a server",
      body: "Register a remote MCP server, or upload your own and let Nasiko build it.",
    },
    {
      target: "mcp-toolbar",
      title: "Narrow the list",
      body: "Show what you can connect or have connected, filter by source, and search (press /).",
    },
    {
      target: "mcp-grid",
      side: "top",
      title: "Connect",
      body: "Each card shows its tools and owner. Connect signs you in; open a server to choose which agents may use it.",
    },
  ],

  sessions: [
    {
      title: "Sessions",
      body: "Every conversation with your agents: what was asked, what it cost, and how it went.",
    },
    {
      target: "page-actions",
      title: "Pick the window",
      body: "Presets or a custom UTC range. The list follows it.",
    },
    {
      target: "sessions-toolbar",
      title: "What needs attention",
      body: "Jump to failing, slow or costly sessions, filter by agent, or pause the live list.",
    },
    {
      target: "sessions-list",
      side: "top",
      title: "The sessions",
      body: "Start, agent, first input, cost and status. Expand a row, or open it for the full trace.",
    },
  ],

  session: [
    {
      title: "A session",
      body: "What happened in one conversation, from the question to every model and tool call.",
    },
    {
      target: "session-narrative",
      title: "What happened",
      body: "A plain summary of the trace. Click a phrase to select the span it describes.",
    },
    {
      target: "session-spans",
      side: "top",
      title: "Every step",
      body: "Each span as a waterfall or a table. Filter spans to find a tool or a model call.",
    },
    {
      target: "session-span-panel",
      side: "left",
      title: "Span details",
      body: "Inputs, outputs, tokens and timing for the selected span. Use ↑ and ↓ to move between spans.",
    },
    {
      target: "page-stepper",
      title: "Next session",
      body: "Step through the list you came from with these arrows, or J and K.",
    },
  ],

  flows: [
    {
      title: "Flows",
      body: "Every request to your agents, including the ones agents make to each other.",
    },
    {
      target: "page-actions",
      title: "Pick the window",
      body: "Flows keep their own window, 7 days to start.",
    },
    {
      target: "flows-charts",
      title: "Requests and timing",
      body: "How many requests came in, and how long finished ones took.",
    },
    {
      target: "flows-table",
      side: "top",
      title: "Each request",
      body: "Question, agent, status and duration. Open one to see every hop on a timeline.",
    },
  ],

  tokenops: [
    {
      title: "TokenOps",
      body: "Where your spend goes: by day, by agent, provider and model, and against the month so far.",
    },
    {
      target: "page-actions",
      title: "Pick the window",
      body: "24 hours to last month, or a custom range. Every panel follows it.",
    },
    {
      target: "tokenops-filters",
      title: "Narrow it down",
      body: "Filter every number by agent, provider or model.",
    },
    {
      target: "tokenops-summary",
      title: "The short version",
      body: "Spend for the window against the period before, and how the month is tracking.",
    },
    {
      target: "tokenops-sections",
      side: "top",
      title: "Dig in",
      body: "Open a section: spend over time, savings, who drives cost, this month, or every metric.",
    },
  ],

  optimization: [
    {
      title: "Optimization",
      body: "How much history Nasiko trims before it reaches the model, and what that saves.",
    },
    {
      target: "optimization-header",
      title: "Window and settings",
      body: "Pick the window for the savings, or jump to your own settings.",
    },
    {
      target: "optimization-lead",
      title: "The answer",
      body: "How much is being saved and what the numbers cover. Click a bar to narrow the biggest senders to that day.",
    },
    {
      target: "optimization-settings",
      side: "top",
      title: "Your settings",
      body: "How history is chosen for your chats, and how much of it is carried.",
    },
  ],

  harnesses: [
    {
      title: "Harnesses",
      body: "Your coding harnesses (like Claude Code or Codex): sessions, models and estimated cost.",
    },
    {
      target: "harnesses-header",
      title: "Window and compare",
      body: "Pick the window, and compare it with the period before.",
    },
    {
      target: "harnesses-summary",
      title: "Your usage",
      body: "What you did and what it cost, with daily activity and top models.",
    },
    {
      target: "harnesses-cards",
      side: "top",
      title: "Each harness",
      body: "Sessions and cost per harness. Pick one to filter the page to it.",
    },
  ],

  settings: [
    {
      title: "Settings",
      body: "Your workspace's defaults and its secrets. Your own appearance and password are in Account settings.",
    },
    {
      target: "settings-sections",
      side: "right",
      title: "Sections",
      body: "General, flow limits, optimization tiers and the registry, then Secrets.",
    },
    {
      target: "settings-form",
      side: "top",
      title: "One form",
      body: "Every workspace section is one form.",
    },
    {
      target: "settings-save",
      side: "top",
      title: "One Save",
      body: "Saves every section at once. It says when you have unsaved changes.",
    },
    {
      target: "secrets-list",
      side: "top",
      title: "Your secrets",
      body: "API keys, stored encrypted and masked. Show and Copy read the value only when you click.",
    },
    {
      target: "secrets-add",
      side: "top",
      title: "Add a secret",
      body: "A name and a value. The router can then use it as a key.",
    },
  ],

  build: [
    {
      title: "A build",
      body: "One deploy, from the upload or clone to a running agent.",
    },
    {
      target: "page-actions",
      title: "Open the agent",
      body: "Once the build is done, go straight to the agent it made.",
    },
    {
      target: "build-outcome",
      title: "How it went",
      body: "Running, not started, or failed. A failed build says what went wrong and offers to deploy again.",
    },
    {
      target: "build-stages",
      side: "top",
      title: "Each stage",
      body: "Queued, building the image, deploying, running. This page updates live, and the build keeps going if you leave.",
    },
  ],

  workflow: [
    {
      title: "A workflow",
      body: "Its steps, which agent runs each one, and its runs.",
    },
    {
      target: "wf-steps",
      side: "top",
      title: "The steps",
      body: "Each step is a task for one agent, run in order. Each one gets the previous step's result.",
    },
    {
      target: "wf-plan",
      title: "Describe it instead",
      body: "Say what the workflow should do and Generate plan drafts the steps for you.",
    },
    {
      target: "wf-run",
      side: "top",
      title: "Run or edit",
      body: "Run starts it now and opens the run. Edit changes its steps.",
    },
    {
      target: "wf-danger",
      side: "top",
      title: "Delete",
      body: "Removes the workflow. Runs that already happened are not affected.",
    },
    {
      target: "page-stepper",
      title: "Next workflow",
      body: "Step through the list you came from with these arrows, or J and K.",
    },
  ],

  "workflow-new": [
    {
      title: "New workflow",
      body: "Chain agents into one job. Describe it and let Nasiko draft the steps, or write them yourself.",
    },
    {
      target: "wf-name",
      title: "Name it",
      body: "A name your team will recognise in the list and in runs.",
    },
    {
      target: "wf-plan",
      title: "Describe it",
      body: "Say what should happen, then Generate plan. You can edit every step it drafts.",
    },
    {
      target: "wf-steps",
      side: "top",
      title: "The steps",
      body: "Add, reorder (drag the grip, or use the arrow keys on it) and remove steps, each with its task and agent.",
    },
    {
      target: "wf-save",
      side: "top",
      title: "Save",
      body: "Save it as a draft to test first, or deploy it ready to run.",
    },
  ],

  "mcp-server": [
    {
      title: "An MCP server",
      body: "One server: its tools, your connection to it, and which agents may use it.",
    },
    {
      target: "mcp-detail-header",
      title: "At a glance",
      body: "Name, status, id, version and owner.",
    },
    {
      target: "mcp-detail-actions",
      side: "left",
      title: "Connect",
      body: "Connect or disconnect your account. The ⋯ menu copies its id or link.",
    },
    {
      target: "mcp-detail-tabs",
      title: "Tabs",
      body: "Agents is where you choose which agents may call its tools, and whether each tool is allowed, asks first or is blocked.",
    },
    {
      target: "mcp-detail-overview",
      side: "top",
      title: "Its tools",
      body: "Details, your connection, and every tool this server offers.",
    },
  ],

  flow: [
    {
      title: "A flow",
      body: "One request, followed through every agent it reached.",
    },
    {
      target: "flow-narrative",
      title: "What happened",
      body: "The flow in one paragraph.",
    },
    {
      target: "flow-kpis",
      title: "The numbers",
      body: "Status, duration and when it started, plus steps, agents and any wait for a human.",
    },
    {
      target: "flow-view",
      title: "Timeline or table",
      body: "See every call on a swimlane, or as a table. Critical path shows what set the total time.",
    },
    {
      target: "page-stepper",
      title: "Next flow",
      body: "Step through the list you came from with these arrows, or J and K.",
    },
  ],

  account: [
    {
      title: "Account settings",
      body: "Yours alone: how Nasiko looks in this browser, your password, and the keys Nasiko answers.",
    },
    {
      target: "settings-sections",
      side: "right",
      title: "Sections",
      body: "Appearance, Password and Keyboard shortcuts.",
    },
    {
      target: "appearance-mode",
      title: "Mode",
      body: "System, Light or Dark, for this browser.",
    },
    {
      target: "appearance-theme",
      side: "top",
      title: "Theme",
      body: "The colour theme. The account menu has a quick switch too.",
    },
    {
      target: "password-form",
      side: "top",
      title: "Change your password",
      body: "Your current password, then the new one twice. Changing it signs you out.",
    },
  ],

  status: [
    {
      title: "Status",
      body: "Whether this page can reach the Nasiko server.",
    },
    {
      target: "status-backend",
      title: "The server",
      body: "Connected, checking or unreachable. When it is unreachable, this says how to start it.",
    },
  ],
};
