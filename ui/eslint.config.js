// The one linter (plan §2.5, §8 Phase 4). `npm run lint` runs it with --max-warnings 0.
import js from '@eslint/js'
import eslintReact from '@eslint-react/eslint-plugin'
import jsxA11y from 'eslint-plugin-jsx-a11y'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import globals from 'globals'
import tseslint from 'typescript-eslint'

/**
 * Each feature's public modules: what another feature may import from it. Anything else is internal.
 * Add a module here in the PR that first needs it from outside, so the shared surface stays visible.
 */
const PUBLIC = {
  agents: [
    'api',
    'types',
    'format',
    'normalize',
    'status',
    'components/AgentCard',
    'components/AgentLink',
    'components/bits',
    'components/dialogs',
    'tuning',
  ],
  chat: [
    'components/OpenChatLink',
    'components/Markdown',
    'components/RequestCard',
    'api',
    'identity',
    'rememberTarget',
    'types',
    'waiting',
    // The harness session ledger maps recorded tool calls (plans/feat-agent-centric.md eng PUBLIC adds for PR1b) and
    // finds its chat past the rail's loaded pages.
    'recorded',
    'sessionLookup',
  ],
  deploy: [
    'AgentBuildsTab',
    'components/DeployAgentButton',
    'components/BuildToasts',
    'copy',
    'follower',
    // The onboarding guide's Bring an agent step hosts the Deploy tabs themselves.
    'UploadTab',
    'GithubTab',
    'RegistryTab',
    'search',
    // The agent hub's deploy markers (plans/feat-agent-centric.md eng): the agent's builds read and its record type.
    'api',
    'types',
  ],
  // Flows (plans/feat-flows.md): the call model and its duration format (the flow narrative), and "Open flow" (F19).
  // The agent hub's Flows list reads the list query (plans/feat-agent-centric.md eng PUBLIC adds).
  flows: ['calls', 'precision', 'components/OpenFlowLink', 'paused', 'api'],
  harnesses: ['types', 'rollup', 'copy', 'api', 'constants', 'liveIndividual'],
  // The agent hub (plans/feat-agent-centric.md PR1a): the agent detail's Overview hosts its timeline.
  // The home's last change links a marker on the hub inside the hub's default window (PR2b).
  hub: ['components/HubTimeline', 'events', 'search'],
  // The agent hub's Tools card counts connectors and blocked tools (plans/feat-agent-centric.md PR1c).
  mcp: ['components/AgentMcpTab', 'api'],
  narrative: ['tokenops', 'trace', 'harness', 'overview', 'flows', 'agentHub', 'home'],
  onboarding: ['api', 'GuideCard', 'GuideHost', 'index', 'logic', 'types'],
  // The product tour (common/src/features/tour): the shell's `_app` route mounts its host.
  tour: ['TourHost'],
  // Context optimization (plans/feat-context-optimization.md eng D1): its rules, copy, the trace's Optimization block
  // (M2) and the /optimization Workspace row (T7). TokenOps' own savings panel is main's (PR #28, /finops/savings).
  optimization: [
    'api',
    'copy',
    'logic',
    'types',
    'components/ContextBlock',
    // The team-name tooltip (B12), on the agent Settings tab's Token optimization title too.
    'components/Codename',
    // The Workspace footer's row, so a layer's slot rows (EE Organization policy) line up with the core's.
    'components/WorkspaceRow',
  ],
  // The agent hub rates one agent with the Overview's own rules (plans/feat-agent-centric.md eng PUBLIC adds).
  // The scoped home (PR2) keeps the Overview's Needs you, first run and Setup guide, its window and its tuning.
  overview: [
    'api',
    'copy',
    'health',
    'search',
    'tuning',
    'components/NeedsYou',
    'components/FirstRun',
    'components/SetupGuide',
  ],
  observability: ['StateCard', 'tuning', 'types', 'spans', 'copy', 'sessions', 'limiter'],
  // Budgets (plans/feat-budget-burn.md eng D1) build their sheet from the router's generic sheet parts.
  router: [
    'components/RoutingCard',
    'api',
    'types',
    'routing',
    'copy',
    'errors',
    'form',
    'focus',
    'components/bits',
    'components/SectionError',
  ],
  // Budgets (plans/feat-budget-burn.md eng D1): R-L10 + burn. Home, Needs you and the hub read budgets through these.
  budgets: [
    'api',
    'types',
    'budgets',
    'copy',
    'tuning',
    'errors',
    'components/BudgetSheet',
    'components/BudgetsSection',
    // Whether this server has budgets (F11): the router's Budgets tab, later Home and the hub.
    'availability',
    // Home's Budget burn panel (F10, B4), lazy-loaded by the home.
    'components/BudgetBurnPanel',
    // The hub's Budget section (B5), lazy-loaded by the hub.
    'components/AgentBudgetSection',
    // The session ledger's budget (B5, F9), lazy-loaded by the ledger.
    'components/SessionBurn',
    // Who reads budgets and which scopes (B6): Home and Needs you follow /budgets' viewer.
    'scope',
    'useBudgetsViewer',
    // The budget's sentence and whether it needs its reader: the hub's Needs you row (B5).
    'sentence',
    // The projection rule (§4.6): Needs you rates the viewer's own budget the way At risk ranks the rest (B4 review).
    'projection',
  ],
  // The hairline rows every Settings page is built from (EE uses them too).
  settings: ['components/SettingRow', 'components/Chosen', 'components/tileStyles'],
  sessions: ['api', 'search', 'LogDrawer'],
  tokenops: [
    'api',
    'types',
    'window',
    'stats',
    'forecast',
    'optimisation',
    'csv',
    'attribution',
    'series',
    // The budget burn's Table view shares the chart panels' table shell (plans/feat-budget-burn.md F1).
    'components/ChartTable',
  ],
  // The session ledger (PR1b) shares the session pages' breadcrumb.
  trace: ['api', 'Waterfall', 'Crumbs'],
  workflows: [],
}

/** Raw controls outside components/ui (Phase 1 guard): shadcn primitives or shared composites instead. */
const RAW = 'button|table|details|input|select|textarea'
/** ARIA widget roles a primitive already implements. */
const ROLES =
  'tab|tablist|tabpanel|menu|menuitem|menubar|radiogroup|radio|listbox|option|switch|checkbox|combobox|dialog|grid|gridcell|tree|treeitem|table|row|cell|columnheader|rowheader'

const noRawElements = [
  {
    selector: `JSXOpeningElement[name.name=/^(${RAW})$/]`,
    message: 'Use the shadcn primitive (components/ui) or a shared composite, not a raw control.',
  },
  {
    selector: `JSXAttribute[name.name='role'][value.value=/^(${ROLES})$/]`,
    message: 'Hand-rolled ARIA widget: use the shadcn primitive that implements this role.',
  },
]

const banned = [
  {
    selector: "JSXAttribute[name.name='dangerouslySetInnerHTML']",
    message: 'No raw HTML: render React nodes (agent markdown goes through components/Markdown).',
  },
  {
    selector: "CallExpression[callee.name='fetch']",
    message: 'Use apiFetch/apiData (common/src/lib/api/client.ts), not bare fetch.',
  },
  {
    selector: "MemberExpression[object.name=/^(window|globalThis)$/][property.name='history']",
    message: 'Navigate with the router (useNavigate / Link), not history.*.',
  },
  {
    selector:
      "MemberExpression[object.type='MemberExpression'][object.object.type='MetaProperty'][object.property.name='env'][property.name!=/^(DEV|MODE|PROD)$/]",
    message: 'Read configuration through src/lib/env.ts, not import.meta.env.',
  },
  {
    selector:
      'Literal[value=/(^|[\\s"\'`:])text-primary(?![-\\w])/], TemplateElement[value.raw=/(^|[\\s"\'`:])text-primary(?![-\\w])/]',
    message:
      'Accent-coloured text is text-primary-text (contrast on every theme), never bare text-primary.',
  },
]

/** The core (and the OSS app) never imports an edition layer: EE code must not reach an OSS build (plan §10.1). */
const NO_LAYERS = {
  group: ['@ee/*', '@mt/*'],
  message:
    'The core never imports an edition layer; add a slot (common/src/app/edition.ts) instead.',
}

const restrictedImports = (...patterns) => [
  'error',
  {
    paths: [{ name: 'framer-motion', message: 'Import from motion/react.' }],
    patterns: [
      { group: ['@radix-ui/*'], message: 'Import from the radix-ui meta-package.' },
      NO_LAYERS,
      ...patterns,
    ],
  },
]
/** A layer's own code may import itself (`@ee/*`) and any core module; the core still never imports it. */
const layerImports = [
  'error',
  {
    paths: [{ name: 'framer-motion', message: 'Import from motion/react.' }],
    patterns: [
      { group: ['@radix-ui/*'], message: 'Import from the radix-ui meta-package.' },
      { group: ['@mt/*'], message: 'EE never imports the multi-tenant layer (it stacks on EE).' },
    ],
  },
]

const featureRule = (name) => {
  const others = Object.keys(PUBLIC).filter((f) => f !== name)
  return {
    files: [`common/src/features/${name}/**/*.{ts,tsx}`],
    ignores: ['**/*.test.{ts,tsx}'],
    rules: {
      'no-restricted-imports': restrictedImports(
        ...others.map((f) => ({
          regex: `^@/features/${f}/(?!(${PUBLIC[f].join('|')})$)`,
          message: `Internal to ${f}. Import one of its public modules (eslint.config.js PUBLIC.${f}) or move the code to src/lib or src/components.`,
        })),
      ),
    },
  }
}

export default tseslint.config(
  {
    ignores: [
      '**/dist/',
      'coverage/',
      'public/',
      '.claude/',
      // The design playground (plans/feat-design-playground.md §5): dev-only tooling, never shipped, and its
      // shell is raw elements by design.
      'design/',
      'ee/web/src/genui/lang/',
      '**/routeTree.gen.ts',
      'common/src/lib/api/schema.gen.ts',
      '**/*.js',
    ],
  },
  js.configs.recommended,
  tseslint.configs.recommended,
  reactHooks.configs.flat.recommended,
  jsxA11y.flatConfigs.recommended,
  {
    files: ['**/*.{ts,tsx}'],
    languageOptions: {
      globals: globals.browser,
      parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname },
    },
    plugins: { 'react-refresh': reactRefresh, '@eslint-react': eslintReact },
    rules: {
      '@typescript-eslint/no-unused-vars': [
        'error',
        { argsIgnorePattern: '^_', varsIgnorePattern: '^_', ignoreRestSiblings: true },
      ],
      // Async code (chat streams, session sync, sign-out) is where a dropped promise hides a bug.
      '@typescript-eslint/no-floating-promises': 'error',
      '@typescript-eslint/no-misused-promises': [
        'error',
        { checksVoidReturn: { attributes: false } },
      ],
      '@typescript-eslint/await-thenable': 'error',
      '@typescript-eslint/no-non-null-assertion': 'error',
      '@eslint-react/no-array-index-key': 'error',
      'react-refresh/only-export-components': ['error', { allowConstantExport: true }],
      'no-console': ['error', { allow: ['warn', 'error'] }],
      // The global only: a local named `history` (a query result) is fine.
      'no-restricted-globals': [
        'error',
        {
          name: 'history',
          message: 'Navigate with the router (useNavigate / Link), not history.*.',
        },
      ],
      'no-restricted-imports': restrictedImports(),
      'no-restricted-syntax': ['error', ...banned, ...noRawElements],
    },
  },
  // Layers: shared code never reaches into a feature; a feature only uses another feature's public modules.
  {
    files: ['common/src/lib/**', 'common/src/components/**', 'common/src/app/**'],
    ignores: ['**/*.test.{ts,tsx}'],
    rules: {
      'no-restricted-imports': restrictedImports({
        group: ['@/features/*', '@/routes/*', '@/mocks/*'],
        message:
          'Shared code (lib, components, app) never imports a feature, a route or the mocks.',
      }),
    },
  },
  ...Object.keys(PUBLIC).map(featureRule),
  // The EE layer builds on the core: any core module (a feature's internals included) and its own `@ee/*`.
  { files: ['ee/**/*.{ts,tsx}'], rules: { 'no-restricted-imports': layerImports } },
  // shadcn primitives: the raw elements live here, and upstream owns their compiler-rule findings.
  {
    files: ['common/src/components/ui/**'],
    rules: {
      'no-restricted-syntax': ['error', ...banned],
      'react-refresh/only-export-components': 'off',
      'react-hooks/purity': 'off',
      'react-hooks/set-state-in-effect': 'off',
      '@eslint-react/no-array-index-key': 'off',
      'jsx-a11y/click-events-have-key-events': 'off',
      'jsx-a11y/no-noninteractive-element-interactions': 'off',
      'jsx-a11y/anchor-has-content': 'off',
    },
  },
  // The two ARIA widgets shadcn has no equivalent for (plan §8 Phase 1 exceptions).
  {
    files: [
      'common/src/features/tokenops/components/MonthHero.tsx',
      'common/src/features/trace/Waterfall.tsx',
    ],
    rules: { 'no-restricted-syntax': ['error', ...banned, noRawElements[0]] },
  },
  // The one fetch wrapper, and the streams it doesn't cover (SSE bodies need the raw Response): chat turns, the
  // agent log tail (`useLogStream`), a build's deploy stream (`useBuildStream`) and EE's Weave generation; API
  // modules, so still no fetch in components.
  {
    files: [
      'common/src/lib/api/client.ts',
      'common/src/features/chat/registry.ts',
      'common/src/features/sessions/api.ts',
      'common/src/features/deploy/api.ts',
      'ee/web/src/genui/conversation/api.ts',
    ],
    rules: {
      'no-restricted-syntax': [
        'error',
        ...banned.filter((b) => !b.selector.includes("'fetch'")),
        ...noRawElements,
      ],
    },
  },
  { files: ['common/src/lib/env.ts'], rules: { 'no-restricted-syntax': 'off' } },
  {
    files: ['common/src/routes/**', '*/*/src/routes/**'],
    rules: { 'react-refresh/only-export-components': 'off' },
  },
  // Tests, mocks and Node scripts: assertions and fixtures may use what app code may not.
  {
    files: ['**/*.test.{ts,tsx}', '*/src/test/**', '*/src/mocks/**', '*/*/src/mocks/**'],
    rules: {
      '@typescript-eslint/no-non-null-assertion': 'off',
      'no-restricted-syntax': 'off',
      'no-console': 'off',
      'react-refresh/only-export-components': 'off',
      '@eslint-react/no-array-index-key': 'off',
    },
  },
  // Node scripts aren't in a tsconfig program: syntax rules only.
  { files: ['scripts/**', '*/*/scripts/**'], ...tseslint.configs.disableTypeChecked },
  {
    files: ['scripts/**', '*/*/scripts/**'],
    rules: { '@typescript-eslint/no-non-null-assertion': 'off' },
  },
  {
    files: [
      'scripts/**',
      '*/*/scripts/**',
      'vite.shared.ts',
      'vitest.config.ts',
      '**/vite.config.ts',
      'common/src/test/live/**',
    ],
    languageOptions: { globals: globals.node },
    rules: { 'no-console': 'off', 'no-restricted-syntax': 'off' },
  },
)
