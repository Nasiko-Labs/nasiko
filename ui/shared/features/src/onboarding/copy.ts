/**
 * Every onboarding string. The flow and wording follow the user's prototype ("Nasiko Console", screen "02 Onboarding"),
 * with the lab's naming: the platform is OpenRuntime, never a "control plane". The Welcome opens with OpenRuntime's
 * narrative, then what it gets you (user review 2026-11-05).
 */
import type { StepId } from './logic'
import type { Persona } from './types'

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
    role: { title: 'Your role', sub: 'Tailor the guide' },
    start: { title: 'Agent and key', sub: 'A first agent, your key' },
    harness: { title: 'Coding harnesses', sub: 'Your sessions, past and new' },
  } satisfies Record<StepId, { title: string; sub: string }>,
  welcome: {
    title: 'Welcome to Nasiko',
    // OpenRuntime's narrative comes back ahead of the capabilities (user review 2026-11-05).
    intro:
      'OpenRuntime runs, observes, secures and pays for AI agents on any framework. Work flows through it in four stages.',
    // The four stages as they were before 2026-11-04 (user review 2026-11-05).
    stages: [
      { title: 'Connect', line: 'Models, MCP tools and channels' },
      { title: 'Build', line: 'Import or create agents' },
      { title: 'Run', line: 'Orchestrate tasks end to end' },
      { title: 'Govern', line: 'Budgets, guardrails and traces' },
    ],
    stagesLabel: 'How work flows through OpenRuntime',
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
  role: {
    title: "What's your role?",
    intro: "We'll suggest the page you need first.",
    label: 'Your role',
    opens: (page: string) => `Opens ${page}`,
    saveFailed: "Couldn't save your role. Try again.",
    retry: 'Try again',
    personas: {
      developer: {
        title: 'Developer',
        line: 'Build, test and ship agents from the CLI and console.',
      },
      platform_engineer: {
        title: 'Platform engineer',
        line: 'Run the cluster, routing and model providers.',
      },
      finance: { title: 'FinOps / Finance', line: 'Track AI spend, budgets and forecasts.' },
      engineering_manager: {
        title: 'Engineering manager',
        line: 'See which coding harnesses your teams use and what they cost.',
      },
      product_manager: {
        title: 'Product manager',
        line: 'Ask agents about users, funnels and feedback.',
      },
      data_analyst: {
        title: 'Data analyst',
        line: 'Follow usage and spend across agents and models.',
      },
      support_lead: { title: 'Support lead', line: 'Triage tickets and answer customers faster.' },
      sre: { title: 'SRE / On-call', line: 'Debug slow or failing agent runs from traces.' },
      leadership: { title: 'Leadership', line: 'One view of adoption, spend and results.' },
    } satisfies Record<Persona, { title: string; line: string }>,
  },
  start: {
    title: 'Bring an agent, then add your key',
    intro:
      'Every agent calls the model with your own provider key, through the gateway that meters every token, so the key is the one thing you need.',
  },
  model: {
    title: '2. Your model key',
    intro: 'It becomes your default model. Add more providers later in the LLM router.',
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
    noProviders: 'No model providers are available on this server yet.',
    loadFailed: 'the model providers',
    saveFailed: (reason: string) => `Couldn't connect: ${reason}`,
  },
  agent: {
    title: '1. Your first agent',
    intro: 'Deploy prebuilt agents, or bring your own.',
    // Pre-seeded agents to deploy, in place of chatting with a running one (meeting 2026-11-06).
    seeded: 'Prebuilt',
    seededLabel: 'Prebuilt agents to deploy',
    seededLead: (max: number) =>
      `Pick up to ${max}. The one with the best record is always included.`,
    picked: (n: number, max: number) => `${n} of ${max} picked`,
    full: 'Uncheck one to swap it for another.',
    included: 'Always included',
    record: (runs: number, success: number) =>
      `${runs.toLocaleString('en-US')} runs · ${Math.round(success * 100)}% succeeded`,
    deploy: (n: number) => (n === 1 ? 'Deploy 1 agent' : `Deploy ${n} agents`),
    deploying: (names: readonly string[]) =>
      names.length === 1
        ? `${names[0]} is on its way. It shows up in Agents once it's running.`
        : `${names.slice(0, -1).join(', ')} and ${names.at(-1)} are on their way. They show up in Agents once they're running.`,
    openAgents: 'Open Agents',
    upload: 'Upload a zip',
    github: 'GitHub',
    registry: 'Registry',
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
      signIn: 'Sign in',
      signInSso: 'Sign in with your company account',
    },
    copy: (what: string) => `Copy: ${what}`,
    after: 'Your next session in any of them shows up in Coding harnesses, next to your agents.',
    // Ways on from the guide's end (user request 2026-11-05): more than the harnesses.
    next: 'Or jump straight in',
    choices: {
      harnesses: { title: 'Open Coding harnesses', line: 'Your sessions and what they cost' },
      agents: { title: 'Open Agents', line: 'Every agent: its health, spend and settings' },
      chat: { title: 'Chat with an agent', line: 'Ask a running agent, and watch its cost add up' },
    },
  },
  card: {
    // The Overview card's rows: the model and agent rows both open the guide's combined step.
    model: { title: 'Connect a model', sub: 'Provider and keys' },
    agent: { title: 'Deploy an agent', sub: 'Zip, GitHub or registry' },
    title: 'Setup guide',
    intro: 'Finish setting up your workspace. Only your model key is required.',
    start: 'Start guide',
    resume: 'Resume guide',
    done: 'Done',
    todo: 'Not yet',
    progress: (done: number, total: number) => `${done} of ${total} done`,
  },
}
