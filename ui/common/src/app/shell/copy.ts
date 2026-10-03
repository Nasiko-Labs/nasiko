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
  openMenu: 'Open navigation',
  collapse: 'Collapse',
  expand: 'Expand',
  collapseShortcut: (mac: boolean) => (mac ? '⌘B' : 'Ctrl+B'),
  nav: {
    fleet: 'Fleet',
    connect: 'Connect',
    observe: 'Observe',
    lab: 'Lab',
    overview: 'Overview',
    chat: 'Chat',
    agents: 'Agents',
    router: 'LLM router',
    mcp: 'MCP servers',
    workflows: 'Workflows',
    sessions: 'Sessions',
    tokenops: 'TokenOps',
    compactTools: 'Compact tools',
    harnesses: 'Harnesses',
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
  notFound: {
    title: 'Page not found',
    body: "There's nothing at this address. It may have moved, or the link is mistyped.",
    home: 'Back to Overview',
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
  account: {
    menu: (name: string) => `Account: ${name}`,
    unavailable: 'Account unavailable',
    loading: 'Loading account…',
    loadingHint: 'Loading your account. Sign out is available once it loads.',
    retry: 'Retry',
    signOut: 'Sign out',
    signingOut: 'Signing out…',
    settings: 'Settings',
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
