/**
 * Every onboarding string. The flow and wording follow the user's prototype ("Nasiko Console", screen "02 Onboarding"),
 * with the lab's naming: the platform is OpenRuntime, never a "control plane". The Welcome leads with cost (user review
 * 2026-11-04).
 */
import type { StepId } from './logic'
import type { Persona } from './types'

export const copy = {
  guide: {
    title: 'Starting guide',
    meta: '3 minutes · have an API key ready',
    rail: 'Getting started',
    step: (n: number, of: number) => `Step ${n} of ${of}`,
    skipGuide: 'Skip guide',
    close: 'Close the guide',
    back: 'Back',
    getStarted: 'Get started',
    continue: 'Continue',
    finish: 'Go to Home',
    left: (minutes: number) =>
      minutes
        ? `About ${minutes} min left · skip anytime`
        : 'All set · every answer can be changed',
  },
  steps: {
    welcome: { title: 'Welcome', sub: 'What Nasiko does for you' },
    role: { title: 'Your role', sub: 'Tailor the guide' },
    start: { title: 'Key and agent', sub: 'Your key, a first chat' },
    optimize: { title: 'Spend less', sub: 'Trim what agents send' },
    ready: { title: 'Ready', sub: 'Start exploring' },
  } satisfies Record<StepId, { title: string; sub: string }>,
  welcome: {
    title: 'Welcome to Nasiko',
    intro: 'Run every agent in one place, see what each one costs, and spend less on them.',
    does: 'What Nasiko does',
    get: 'What you get',
    // Each row is a capability and its outcome, in the order the guide sets them up: run, measure, cut, debug, chat.
    rows: [
      {
        does: 'Runs agents from any framework',
        get: 'Every agent in one place, with its code unchanged',
      },
      { does: 'Meters every model call', get: 'The cost of each agent, model and person' },
      { does: 'Routes each call to a model tier', get: 'Cheaper models for easy calls' },
      { does: 'Trims prompts and chat history', get: 'Fewer tokens billed per call' },
      { does: 'Traces every run', get: 'Why a run was slow, failed or cost what it did' },
      { does: 'Picks the right agent in chat', get: 'Ask without choosing an agent first' },
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
    title: 'Add your key, then chat with an agent',
    intro:
      'Nasiko provides no model credits: every agent calls the model with your own provider key, through the gateway that meters every token. So the key comes first.',
  },
  model: {
    title: '1. Your model key',
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
    vault: 'The key is saved as one of your secrets and never shown again.',
    noProviders: 'No model providers are available on this server yet.',
    loadFailed: 'the model providers',
    saveFailed: (reason: string) => `Couldn't connect: ${reason}`,
  },
  agent: {
    title: '2. Your first agent',
    intro: 'Chat with one that is already running, or bring your own.',
    try: 'Running now',
    tryLabel: 'Agents running now',
    noExample: 'Ask it what it can do.',
    chat: 'Chat',
    chatWith: (name: string) => `Chat with ${name}`,
    // Chat waits for a key: with no Nasiko credits, an agent answers nothing without one.
    needsKey: 'Add your key above to chat. Every agent calls the model with it.',
    upload: 'Upload a zip',
    github: 'GitHub',
    registry: 'Registry',
    lifecycle: 'Lifecycle',
    stages: ['Source', 'Build', 'Deploy', 'Live'],
    started: 'Your agent is on its way. We will let you know when the build finishes.',
    openBuild: 'Open the build',
    openAgent: 'Open the agent',
  },
  optimise: {
    title: 'Cut what your agents spend',
    intro:
      'Agents often send the model more than it needs, like full tool output or the whole chat so far. Each switch below cuts one of these before the call is billed. Your own messages are never changed.',
    items: [
      {
        title: 'Smaller prompts',
        line: 'Shortens long tool output, such as logs, JSON and diffs, before the model reads it.',
        where: 'Agent → Settings → Token optimization',
      },
      {
        title: 'Shorter chat history',
        line: 'Carries a relevant slice of the conversation on each message instead of all of it.',
        // Chat context moved to the Optimization page (integration 2026-10-05, ledger B11).
        where: 'Optimization → Your settings (PACMS, Top-K or Last-K)',
      },
      {
        title: 'Less code written',
        line: 'For coding agents: check for an existing solution before writing new code.',
        where: 'Agent → Settings → Coding agent behavior → Minimal-code mode',
      },
      {
        title: 'A smaller model when it will do',
        line: 'Set tier models on a config and the router picks one per call, so easy calls skip the biggest model.',
        where: 'LLM router → Configs → Tier models',
      },
    ],
    beta: 'These are new. Try one on a single agent first. You can turn it off anytime, and changes apply from the next message.',
    cta: 'Open agent settings',
    savings: 'See what they save',
    skip: 'I will do this later',
  },
  ready: {
    title: 'Your workspace is ready',
    intro: 'Here is what we set up. Everything can be changed later.',
    role: 'Role',
    model: 'Model',
    agent: 'Agent',
    skipped: 'Skipped',
    building: 'Building',
    edit: (what: string) => `Edit ${what.toLowerCase()}`,
    editShort: 'Edit',
    next: 'What next',
    chat: 'Chat with an agent',
    chatLine: 'Ask a running agent anything, and watch its tokens and cost add up.',
    chatNeedsKey: 'Add your model key first: every agent needs it.',
    agents: 'Open Agents',
    agentsLine: 'Every agent here: its health, spend and settings',
    open: (page: string) => `Open ${page}`,
    openLine: 'Your suggested first screen',
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
