/** Every user-facing string of the app shell (sidebar, theme menu, sign out). */
export const copy = {
  brand: 'Nasiko',
  brandTooltip: '',
  navLabel: 'Main',
  // The drill-in panel's way back (Chat, Settings): it reads "Back"; its name (and tooltip) says where to: the page
  // the user came from, or, opened directly, the app nav (the page stays where it is). The visible word leads the name.
  back: 'Back',
  backTo: (page: string) => `Back to ${page}`,
  backToMenu: 'Back to main menu',
  statusPage: 'Status',
  skipToContent: 'Skip to content',
  /** The top bar's help button: starts the current page's tour. */
  tour: 'Tour this page',
  openMenu: 'Open navigation',
  collapse: 'Collapse',
  expand: 'Expand',
  collapseShortcut: (mac: boolean) => (mac ? '⌘B' : 'Ctrl+B'),
  nav: {
    fleet: 'Fleet',
    connect: 'Connect',
    observe: 'Observe',
    views: 'Views',
    org: 'Organization',
    lab: 'Lab',
    home: 'Home',
    chat: 'Chat',
    agents: 'Agents',
    router: 'LLM router',
    mcp: 'MCP servers',
    workflows: 'Workflows',
    sessions: 'Sessions',
    flows: 'Flows',
    tokenops: 'TokenOps',
    monitor: 'Monitor',
    optimise: 'Optimize',
    harnesses: 'Coding harnesses',
    settings: 'Settings',
  },
  routeError: {
    title: "This page couldn't load",
    body: 'Something went wrong while showing this page. Try again, or go back.',
    noAccess: "You don't have access to this page",
    noAccessBody: 'Ask an admin or the owner for access.',
    tryAgain: 'Try again',
    loading: 'Loading page',
  },
  // lib/staleBuild.ts: a tab older than the running build. A new version is said only when the server serves one.
  staleBuild: {
    notice: 'A new version of OpenRuntime is available. Save any changes, then reload.',
    reload: 'Reload',
    tryAgain: 'Try again',
    checking: { title: 'Loading page', body: "Checking why this page couldn't load." },
    reloading: { title: 'OpenRuntime was updated', body: 'Loading the new version…' },
    held: { title: 'OpenRuntime was updated', body: 'Save your changes, then reload to continue.' },
    exhausted: {
      title: 'OpenRuntime was updated',
      body: "This page couldn't load its code. Reload to continue.",
    },
    offline: { title: "You're offline", body: 'This page will load when the connection is back.' },
    network: { title: "Couldn't reach OpenRuntime", body: 'Check the connection, then try again.' },
    failed: { title: "This page couldn't load", body: 'Try again in a moment.' },
  },
  notFound: {
    title: 'Page not found',
    body: "There's nothing at this address. It may have moved, or the link is mistyped.",
    home: 'Back to Home',
    toChat: 'Go to Chat',
  },
  status: {
    checking: 'Checking…',
    connected: 'Connected',
    unreachable: 'Server unreachable',
    // Visible label beside the MOCK DATA badge: the full text truncated to "Server unre…" at 240 px (QA ISSUE-004).
    unreachableShort: 'Unreachable',
    unreachableHint: 'Server unreachable. Open Status for the fix.',
    mockBadge: 'MOCK DATA',
    mockShort: 'M',
    liveBadge: 'LIVE',
    liveShort: 'L',
    mockTitle: 'All API responses come from the local seed (MSW)',
    liveTitle: (partial: readonly string[]) =>
      `Proxied to NASIKO_API_URL${partial.length ? ` · mocked: ${partial.join(', ')}` : ''}`,
  },
  theme: {
    menu: 'Theme',
    modeGroup: 'Mode',
    accentGroup: 'Theme',
    system: 'System',
    light: 'Light',
    dark: 'Dark',
    teal: 'Teal',
    indigo: 'Indigo',
    plum: 'Plum',
    carbon: 'Carbon',
  },
  // The early-access card at the foot of the sidebar, its rail button and the login line (OSS only; WaitlistCta.tsx).
  waitlist: {
    badge: 'Early access',
    title: 'Join the Nasiko waitlist',
    line: 'Get managed OpenRuntime early, with help from the team.',
    cta: 'Join the waitlist',
    newTab: '(opens in a new tab)',
    loginLead: 'New to Nasiko?',
  },
  // The free-assessment card above the sidebar footer and its rail row (AssessmentCta.tsx); it replaced the waitlist's.
  assessment: {
    badge: 'Free',
    title: 'Get a free TokenOps assessment',
    line: "Tell us how your agents run. We'll show where the tokens go.",
    cta: 'Request assessment',
  },
  // The sidebar footer's help rows, above the account (AppSidebar.tsx HelpRows).
  help: {
    docs: 'How to use',
    cli: 'CLI guide',
    expert: 'Talk to an expert',
    newTab: '(opens in a new tab)',
  },
  // The desktop app's sign-in, while it runs Nasiko on this computer (ForgotPassword.tsx).
  forgotPassword: {
    link: 'Forgot password?',
    lead: "The password can't be recovered. You can reset Nasiko on this computer and start over.",
  },
  account: {
    menu: (name: string) => `Account: ${name}`,
    unavailable: 'Account unavailable',
    loading: 'Loading account…',
    loadingHint: 'Loading your account. Sign out is available once it loads.',
    retry: 'Retry',
    signOut: 'Sign out',
    signingOut: 'Signing out…',
    settings: 'Account settings',
    sidebarTour: 'Tour the sidebar',
  },
  login: {
    title: 'Sign in to Nasiko',
    // The decorative showcase beside the form (aria-hidden; the h1 names the page). Its chips are `nav` labels.
    showcaseTitle: 'Nasiko',
    // nasiko.com's hero: the lead, then each word in turn, looping (LoginShowcase). In the prototype's order; the
    // chips and the layer stack carry the same words.
    showcaseLead: 'The OpenRuntime for',
    showcaseWords: ['Agents', 'Coding Harnesses', 'Tools', 'Frameworks'],
    showcaseLine: 'Use any harness. Reach any model. Measure every token. Change nothing.',
    username: 'Username',
    password: 'Password',
    usernamePlaceholder: 'Enter username',
    passwordPlaceholder: 'Enter password',
    showPassword: 'Show password',
    // Only reachable where native `required` doesn't stop the submit first.
    usernameRequired: 'Enter your username.',
    passwordRequired: 'Enter your password.',
    submit: 'Sign in',
    submitting: 'Signing in…',
    expired: 'Your session expired. Sign in again to continue.',
    // nasiko.com's own summary (its meta description). Mock mode pre-fills the form and the sidebar badge says MOCK
    // DATA; the local live login (admin / changeme) is in CLAUDE.md.
    tagline:
      'Nasiko helps enterprises account for what their AI agents cost, can access, and do, across every model and provider.',
    passwordChanged: 'Password changed. Please sign in again with your new password.',
    wrongCredentials: 'Wrong username or password.',
    rateLimited: 'Too many attempts. Wait a minute and try again.',
    // auth/service.rs (ea233d20): 3 failed sign-ins lock the account for 15 minutes (429 `account_locked`). Worded
    // without saying the account exists, like the 401 message.
    locked: (minutes: number) =>
      `Too many failed sign-ins. Try again in ${minutes} minute${minutes === 1 ? '' : 's'}.`,
    unreachable: "Can't reach nasiko-server. Is `just run-stack` running?",
    timedOut: 'The server took too long to answer. Try again in a moment.',
    failed: 'Login failed. Check the server logs.',
    // The login cookie is HttpOnly: only the server can end it, so a failed logout leaves this
    // browser signed in. Say so plainly (review: adversarial + Codex + red team).
    signOutFailed:
      "Sign out didn't finish: the server didn't answer, so this browser may still be signed in. Try again before you leave this device.",
    tryAgain: 'Try again',
    tryingAgain: 'Trying again…',
    tryAgainFailed: 'Still no answer from the server.',
  },
} as const
