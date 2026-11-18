/**
 * Every onboarding string. The flow and wording follow the user's prototype ("Nasiko Console", screen "02 Onboarding"),
 * with the lab's naming: the platform is Open Runtime, never a "control plane". The Welcome opens with Open Runtime's
 * narrative, then what it gets you (user review 2026-11-05).
 */
import type { StepId } from './logic'

export const copy = {
  guide: {
    title: 'Starting guide',
    meta: '2 minutes · have an API key ready',
    rail: 'Getting started',
    step: (n: number, of: number) => `Step ${n} of ${of}`,
    skipGuide: 'Skip guide',
    close: 'Close the guide',
    back: 'Back',
    getStarted: 'Get started',
    continue: 'Continue',
    continueIn: (seconds: number) => `Continue (${seconds})`,
    finish: 'Finish',
    left: (minutes: number) =>
      minutes
        ? `About ${minutes} min left · skip anytime`
        : 'All set · every answer can be changed',
  },
  steps: {
    welcome: { title: 'Welcome', sub: 'What Nasiko does for you' },
    start: { title: 'Agent and key', sub: 'A first agent, your key' },
    harness: { title: 'Coding harnesses', sub: 'Your sessions, past and new' },
  } satisfies Record<StepId, { title: string; sub: string }>,
  welcome: {
    title: 'Welcome to Nasiko',
    // Open Runtime's narrative comes back ahead of the capabilities (user review 2026-11-05). It meters spend, never
    // "pays for" agents: that read as Nasiko covering the user's model costs (NAS-1592). No lead-in to the stages
    // below: "work flows through it in four stages" was unclear, and the row stands on its own.
    intro:
      'Open Runtime runs, observes and secures AI agents on any framework, and meters what they spend.',
    // The four stages as they were before 2026-11-04 (user review 2026-11-05).
    stages: [
      { title: 'Connect', line: 'Models, MCP tools and channels' },
      { title: 'Build', line: 'Import or create agents' },
      { title: 'Run', line: 'Orchestrate tasks end to end' },
      { title: 'Govern', line: 'Budgets, guardrails and traces' },
    ],
    stagesLabel: 'How work flows through Open Runtime',
    does: 'What Nasiko does',
    get: 'What you get',
    // One row per category (user review 2026-11-05: six rows repeated each other): run, measure, cut, harnesses.
    rows: [
      {
        does: 'Runs agents from any framework',
        get: 'Every agent in one place, with its code unchanged',
      },
      {
        does: 'Meters and traces every call',
        get: 'What each agent, model and person costs, and why a run failed',
      },
      {
        does: 'Routes and trims each call',
        get: 'Cheaper models and fewer tokens for the same answer',
      },
      {
        does: 'Records your coding harnesses',
        get: 'Claude Code, Codex and Cursor sessions beside your agents',
      },
    ],
  },
  start: {
    title: 'Bring an agent, then add your key',
    intro: 'Every model call goes through the gateway that meters every token.',
  },
  model: {
    title: '2. Your model key',
    // Starter pack agents run on the platform's model (Free plane: the seed agents' daily allowance); only a
    // user's own agents need their key (user request 2026-11-13).
    intro:
      'Starter pack agents are free to use. For your own agents, bring your own model key: it becomes your default model, and you can add more providers later in the LLM router.',
    /** Without the Starter pack (an upgraded workspace): every agent is the user's own. */
    introOwn:
      'Your agents call the model with your own key. It becomes your default model; add more providers later in the LLM router.',
    label: 'Model provider',
    models: (n: number) => `${n} model${n === 1 ? '' : 's'}`,
    key: (provider: string) => `${provider} API key`,
    // Each provider's key format, as the prototype shows it.
    placeholder: { openai: 'sk-proj-…', anthropic: 'sk-ant-…', gemini: 'AIza…' } as Record<
      string,
      string
    >,
    placeholderOther: 'Paste your API key',
    showKey: 'Show key',
    // The provider's catalog models as suggestions, or any name typed in (the LLM router's field, SuggestInput).
    model: 'Model',
    modelPlaceholder: 'Pick or type a model',
    // Where each provider creates keys; the ? beside the key field links it.
    keyPage: {
      openai: 'https://platform.openai.com/api-keys',
      anthropic: 'https://platform.claude.com/settings/keys',
      gemini: 'https://aistudio.google.com/apikey',
      openrouter: 'https://openrouter.ai/settings/keys',
    } as Record<string, string>,
    help: {
      label: 'Where do I get a key?',
      title: (provider: string) => `Get an API key from ${provider}`,
      open: (provider: string) => `Open ${provider}'s API keys page`,
      signIn: 'and sign in.',
      steps: [
        'Create a new key and copy it. It is shown only once.',
        'Paste it here and press Connect.',
      ],
      billing: 'Calls are billed to the account that owns the key.',
    },
    connect: 'Connect',
    connecting: 'Connecting…',
    connected: (what: string) =>
      `Connected · ${what} is now your default model through the gateway`,
    keyEmpty: 'Paste a key first.',
    modelEmpty: 'Pick a model first.',
    vault: 'The key is saved as one of your secrets and never shown again.',
    // Where the saved key lives afterwards (user request 2026-11-12): named, not linked, so the guide isn't left.
    manageSecrets: 'You can manage it any time in Settings → Secrets.',
    noProviders: 'No model providers are available on this server yet.',
    loadFailed: 'the model providers',
    saveFailed: (reason: string) => `Couldn't connect: ${reason}`,
  },
  agent: {
    title: '1. Your first agent',
    intro: 'Start with the agents we give you, or bring your own.',
    /** Without the Starter pack (an upgraded workspace): the user brings their own. */
    introOwn: 'Bring your own agent: from a registry, GitHub or a zip.',
    // The agents every new workspace is given, listed, not picked (user request 2026-11-12).
    seeded: 'Starter pack',
    seededLabel: 'Your starter pack',
    giveaway: (n: number) =>
      `Here's your starter pack: ${n} agents, ready to go. When you want more, the Marketplace has plenty to install.`,
    record: (runs: number, success: number) =>
      `${runs.toLocaleString('en-US')} runs · ${Math.round(success * 100)}% succeeded`,
    // Each deploy started from the guide's tabs, live (NAS-1450; user request 2026-11-12): a clock while it runs, then
    // its outcome.
    progressLabel: 'Your deploys',
    progress: {
      deploying: (elapsed: string, usual: string) => `Deploying · ${elapsed} · ${usual}`,
      running: 'Running',
      notRunning: 'Deployed, but it did not start',
      failed: (reason: string) => `Failed: ${reason}`,
    },
    keepGoing: 'You can keep going; it carries on while you finish the guide.',
    /** A followed build's failure carries no reason of its own; the build page has the log. */
    buildFailed: 'The build failed',
    /** The guide's footer, on the steps after the agent one. */
    summary: {
      deploying: (n: number) => (n === 1 ? 'Deploying 1 agent…' : `Deploying ${n} agents…`),
      running: (n: number) => (n === 1 ? '1 agent running' : `${n} agents running`),
      failed: (n: number, of: number) =>
        of === 1 ? "Your agent didn't start" : `${n} of ${of} agents didn't start`,
    },
    openAgents: 'Open Agents',
    upload: 'Upload a zip',
    github: 'GitHub',
    registry: 'Registry',
    // How to bring an agent from the public registry (user request 2026-11-09), above the Deploy page's own form.
    registryHow: {
      title: 'Copy an agent from the Nasiko registry',
      site: 'registry.nasiko.com',
      siteUrl: 'https://registry.nasiko.com',
      open: 'Open',
      openRest: 'and pick an agent.',
      copyStep: 'In its Install section, copy the reference after',
      pull: 'docker pull',
      example: 'registry.nasiko.com/acme/support-bot:1.2.0',
      paste: 'Paste it below and press Import and deploy.',
      label: 'How to copy from the registry',
    },
    lifecycle: 'Lifecycle',
    stages: ['Source', 'Build', 'Deploy', 'Live'],
    started: 'Your agent is on its way. We will let you know when the build finishes.',
    openBuild: 'Open the build',
    openAgent: 'Open the agent',
  },
  harness: {
    title: 'Bring in your coding harnesses',
    intro:
      'Claude Code, Codex, Cursor and the rest report every session to Nasiko: what it cost, which models it used, and the sessions you had before today.',
    steps: {
      install: 'Install the CLI',
      connect: 'Connect it to this server',
    },
    copy: (what: string) => `Copy: ${what}`,
    // `nasiko connect` signs in and asks about the coding agents itself (user request 2026-11-15): no sign-in step.
    after:
      'Connecting signs you in, then asks which coding agents to connect; one connected to another server is offered to move here. Your next session in any of them shows up in Coding harnesses, next to your agents.',
    // Ways on from the guide's end (user request 2026-11-05): more than the harnesses.
    next: 'Or jump straight in',
    choices: {
      harnesses: { title: 'Open Coding harnesses', line: 'Your sessions and what they cost' },
      agents: { title: 'Open Agents', line: 'Every agent: its health, spend and settings' },
      chat: { title: 'Chat with an agent', line: 'Ask a running agent, and watch its cost add up' },
    },
  },
  card: {
    // The Overview card's rows: the model and agent rows both open the guide's combined step. Each says what it asks
    // for and why, and the first one not done says it is next, never "not yet" (NAS-1448).
    model: {
      title: 'Add your model key',
      sub: 'For your own agents: an API key from OpenAI, Anthropic or another provider',
    },
    agent: { title: 'Deploy an agent', sub: 'Pick a prebuilt one, or bring your own' },
    title: 'Setup guide',
    intro:
      'Two steps to your first agent run. Starter pack agents are free; your own agents call their model with your own API key.',
    start: 'Start guide',
    resume: 'Resume guide',
    done: 'Done',
    next: 'Up next',
    // The agent row while the guide's prebuilt deploy runs, and when it didn't start (NAS-1450).
    deploying: 'Deploying…',
    notStarted: "Didn't start",
    progress: (done: number, total: number) => `${done} of ${total} done`,
    // The header button's own count beside its bar (user request 2026-11-09: progress on the home screen).
    progressShort: (done: number, total: number) => `${done}/${total}`,
  },
}
