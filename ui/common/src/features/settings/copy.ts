/**
 * Every user-facing string on Settings and Secrets (plans/feat-settings.md). Worded as nasiko-cloud-rs `43833316`
 * ui/common/pages/settings-page.js, secrets-page.js and features/secrets-manager.js say it, except where the legacy
 * copy was wrong about the server (marked).
 */
import type { SecretNameProblem } from './logic'
import type { BudgetLevel, ContextStrategy } from './types'

export const copy = {
  title: 'Settings',
  nav: {
    label: 'Settings sections',
    workspace: 'Workspace',
    security: 'Security',
    account: 'Account',
  },
  save: 'Save changes',
  saving: 'Saving…',
  unsaved: 'Unsaved changes',
  saved: 'Settings saved',
  saveFailed: (reason: string) => `Could not save settings: ${reason}`,
  loadWhat: 'settings',
  loading: 'Loading settings',
  required: 'Enter a value.',
  positiveInt: 'Enter a whole number of 1 or more.',

  // Settings → Account → Appearance (/settings/appearance): every user, stored in this browser only (theme.ts).
  appearance: {
    label: 'Appearance',
    sub: 'How Nasiko looks in this browser.',
    mode: 'Mode',
    modeHint: 'System follows your device’s light or dark setting.',
    theme: 'Theme',
    themeHint: 'The colour of buttons, links and the current page. Sign in always uses Carbon.',
  },

  // Settings → Account → Password (/settings/password), worded as nasiko-cloud-rs `43833316`
  // ui/common/features/change-password-modal.js.
  password: {
    label: 'Password',
    sub: 'The password you sign in with. Changing it signs out your other sessions.',
    current: 'Current password',
    currentPlaceholder: 'the password you sign in with now',
    next: 'New password',
    nextPlaceholder: 'a new, unused password',
    confirm: 'Confirm new password',
    confirmPlaceholder: 'retype the new password',
    show: (field: string) => `Show ${field.toLowerCase()}`,
    policy: (min: number, max: number) =>
      `${min}-${max} characters, with an uppercase letter, a lowercase letter, a digit and a symbol.`,
    submit: 'Change password',
    submitting: 'Changing…',
    currentRequired: 'Enter your current password',
    nextRequired: 'Enter a new password',
    problem: {
      bytes: 'Password must be at most 72 bytes',
      short: (min: number) => `Password must be at least ${min} characters`,
      long: (max: number) => `Password must be at most ${max} characters`,
      lowercase: 'Password must contain a lowercase letter',
      uppercase: 'Password must contain an uppercase letter',
      digit: 'Password must contain a digit',
      symbol: 'Password must contain a symbol',
    },
    same: 'New password must differ from the current one',
    mismatch: 'New passwords do not match',
    changed: 'Password changed. Your other sessions have been signed out.',
    failed: 'Could not change password',
    unreachable: 'Could not reach the server. Try again.',
  },

  // Settings → Security → Chat context (/settings/chat-context): the caller's own, worded as nasiko-cloud-rs `35c749af`
  // ui/common/pages/chat-context-page.js.
  chatContext: {
    label: 'Chat context',
    sub: 'How much of your conversation history is carried into each request, and which algorithm picks it. Applies to every chat you send, across all agents.',
    strategy: 'Context strategy',
    strategyHint: 'Which algorithm selects prior messages to carry into a new request.',
    strategies: {
      pacms: 'PACMS (budget-aware, recommended)',
      topk: 'Top-K (relevance ranked)',
      lastk: 'Last-K (most recent)',
    } satisfies Record<ContextStrategy, string>,
    budget: 'History budget',
    budgetHint:
      'How much history the strategy may keep: a token budget under PACMS, an item count under Top-K and Last-K.',
    levels: { low: 'Low', medium: 'Medium', high: 'High' } satisfies Record<BudgetLevel, string>,
    saved: 'Chat context settings saved',
    loadWhat: 'your chat context settings',
    loading: 'Loading chat context settings',
    newerServer: 'Chat context needs a newer OpenRuntime server',
  },

  sections: {
    general: { label: 'General', sub: 'Routing defaults and platform behaviour.' },
    limits: { label: 'Flow limits', sub: 'Cascade guards applied to every inter-agent call.' },
    registry: { label: 'Registry', sub: 'External OCI registry used for agent images.' },
  },

  fields: {
    router_model: {
      label: 'Router model',
      hint: 'Model the routing engine uses to pick an agent for each query (ROUTER_MODEL).',
      placeholder: 'e.g. gpt-4o',
    },
    default_provider: {
      label: 'Default provider',
      hint: 'Provider used when an agent has no LLM config of its own.',
    },
    catalog_tabs: {
      label: 'Agent catalog tabs',
      hint: "Comma-separated agent tags pinned as the catalog's filter tabs. Leave empty to derive tabs from the most common tags across agents.",
      placeholder: 'e.g. devops, finance, support',
    },
    max_flow_depth: {
      label: 'Max call depth',
      hint: "How many agent-to-agent hops one flow may chain before it's rejected.",
    },
    max_flow_fan_out: {
      label: 'Max fan-out',
      hint: 'Maximum agents a single flow may call in total.',
    },
    max_flow_tokens: {
      label: 'Token budget per flow',
      hint: 'Combined prompt + completion tokens a flow may spend.',
    },
    flow_timeout_secs: {
      label: 'Flow timeout (seconds)',
      hint: 'Wall-clock limit for a whole flow.',
    },
    registry_url: {
      label: 'OCI registry URL',
      // Legacy: "Where imported agent images are pulled from." The server adds this URL's host to the registry
      // import's allow list (catalog/import.rs effective_allowed_hosts), live.
      hint: 'Registry imports accept this registry as well as the built-in ones. Only the host counts.',
      placeholder: 'https://registry.example.com',
      invalid: 'Enter a registry host or URL, like https://registry.example.com.',
    },
  },
  providers: [
    ['openai', 'OpenAI'],
    ['anthropic', 'Anthropic'],
    ['gemini', 'Gemini'],
  ] as const,
  providerKeys: {
    label: 'Provider API keys',
    before:
      "Keys aren't stored here — each routing config references one of your encrypted secrets. Manage them on the ",
    router: 'LLM router',
    and: ' and ',
    secrets: 'Secrets',
    after: ' pages.',
  },
  registryCredentials: {
    label: 'Registry credentials',
    body: 'Per-agent pull credentials are issued by the platform, and the cluster-wide build credential comes from BUILD_PUSH_TOKEN — neither is configured from this page.',
  },

  secrets: {
    title: 'Secrets',
    sub: 'API credentials stored in this workspace. Router configs and agents reference secrets by name.',
    // Legacy: "Keys are write only … never read back", beside a list that reveals them. The owner can read their own.
    note: 'Values are encrypted at rest. Configs reference a secret by name; only you can read your own secrets back, one at a time.',
    empty: 'No secrets yet',
    emptyText: 'Add one below; agents and router configs reference secrets by name.',
    loadWhat: 'your secrets',
    loading: 'Loading secrets',
    masked: '••••••••',
    updated: (when: string) => `Updated ${when}`,
    show: (n: string) => `Show the value of ${n}`,
    hide: (n: string) => `Hide the value of ${n}`,
    copy: (n: string) => `Copy the value of ${n}`,
    delete: (n: string) => `Delete secret ${n}`,
    confirm: 'Delete this secret?',
    usedBy: (configs: string) => `${configs} will lose its provider key.`,
    cancel: 'Cancel',
    confirmDelete: 'Delete',
    name: 'Name',
    namePlaceholder: 'API_KEY',
    value: 'Value',
    valuePlaceholder: 'sk-…',
    add: 'Add secret',
    adding: 'Adding…',
    nameProblem: {
      required: 'Enter a secret name.',
      length: 'Use at most 128 characters.',
      pattern: 'Use A–Z, 0–9 and _ only, starting with a letter or _ — e.g. API_KEY.',
      reserved: 'This name is reserved: it would change how an agent’s container runs.',
    } satisfies Record<SecretNameProblem, string>,
    valueRequired: 'Enter a value.',
    saved: (n: string) => `${n} saved.`,
    saveFailed: (n: string, reason: string) => `Could not save ${n}: ${reason}`,
    deleted: (n: string) => `${n} deleted.`,
    deleteFailed: (n: string, reason: string) => `Could not delete ${n}: ${reason}`,
    readFailed: (n: string, reason: string) => `Could not read ${n}: ${reason}`,
    copied: (n: string) => `${n} copied.`,
    copyFailed: (n: string, reason: string) => `Could not copy ${n}: ${reason}`,
    needsSecureContext: 'Copying needs HTTPS or localhost.',
  },
}
