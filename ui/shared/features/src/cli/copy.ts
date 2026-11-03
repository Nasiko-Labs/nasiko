/**
 * Every string of the CLI guide (`/cli`). Commands come from docs.nasiko.com/reference/cli (fetched 2026-10-22) and the
 * installer from oss/docs/CLI_INSTALL.md; re-check them when the CLI changes.
 */
export const copy = {
  title: 'Nasiko CLI',
  sub: 'Deploy, chat with and observe your agents from the terminal.',
  reference: 'Full CLI reference',
  newTab: '(opens in a new tab)',
  install: {
    title: 'Install',
    line: 'One command, no account and no administrator rights. Run it again to update.',
    unix: 'macOS and Linux',
    windows: 'Windows (PowerShell)',
    unixCommand: 'curl -fsSL https://registry.nasiko.dev/r/nasiko/install | bash',
    windowsCommand: 'irm https://registry.nasiko.dev/r/nasiko/install-ps1 | iex',
  },
  tasks: {
    title: 'What do you want to do?',
  },
  start: {
    title: 'Get started',
    install: 'Install the CLI',
    connect: 'Connect to this cluster',
    signIn: 'Sign in with your Nasiko account',
    deploy: 'Deploy an agent from its folder',
  },
  copyCommand: (what: string) => `Copy: ${what}`,
}

/** The "What do you want to do?" cards: a docs page under reference/cli and one command from it. */
export const TASKS = [
  {
    page: 'setup',
    title: 'Set up and sign in',
    line: 'Run the stack locally or connect to a cluster.',
    command: 'nasiko up',
  },
  {
    page: 'coding-agents',
    title: 'Coding agents',
    line: 'Find Claude Code, Codex and others, and track their usage.',
    command: 'nasiko agents discover',
  },
  {
    page: 'create',
    title: 'Create an agent',
    line: 'Scaffold, build and run an A2A agent locally.',
    command: 'nasiko new',
  },
  {
    page: 'operate',
    title: 'Deploy and operate',
    line: 'Upload, deploy, scale and read logs.',
    command: 'nasiko upload',
  },
  {
    page: 'chat',
    title: 'Chat',
    line: 'Talk to an agent or the orchestrator.',
    command: 'nasiko chat --tui',
  },
  {
    page: 'observe',
    title: 'Observe and TokenOps',
    line: 'Sessions, traces and what each run cost.',
    command: 'nasiko observe sessions',
  },
] as const
