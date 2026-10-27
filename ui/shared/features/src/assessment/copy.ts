/** Every string of the free TokenOps assessment (`/assessment`), and its answer options. */
export const copy = {
  title: 'Free TokenOps assessment',
  badge: 'Free developer assessment',
  headline: 'Get your free assessment scheduled with Nasiko',
  lead: 'Are your AI agents using more tokens than expected? Get a clearer picture of your token spend, model usage and the challenges slowing down your agent workflows.',
  points: [
    'Token usage and cost visibility',
    'Agent workflows and model routing',
    'A direct line to the Nasiko team',
  ],
  form: {
    title: "Let's understand your setup",
    sub: "Whether you build on your own or run agents across a team, we'd like to hear from you.",
    applyingAs: "I'm applying as",
    name: 'Your full name',
    namePlaceholder: 'Enter your name',
    email: 'Email address',
    emailPlaceholder: 'you@example.com',
    company: 'Company',
    companyPlaceholder: 'Where you work',
    agents: 'How many AI agents do you run?',
    spend: 'Estimated monthly token spend',
    spendHint: 'Across all your models and providers. A rough figure is fine.',
    challenges: 'What challenges are you facing?',
    challengesHint: 'Select all that apply.',
    notes: 'Tell us about your setup or biggest blocker',
    notesPlaceholder:
      'Which models, frameworks and providers do you use? What would you most like to solve?',
    required: '(required)',
    submit: 'Continue to scheduling',
    sending: 'Sending your answers…',
    fine: 'We use your answers only to prepare your assessment. Requests are subject to follow-up and scheduling.',
  },
  errors: {
    name: 'Enter your name.',
    email: 'Enter your email address.',
    emailFormat: 'Enter a valid email address, like you@example.com.',
    company: 'Enter your company.',
    agents: 'Pick how many agents you run.',
    spend: 'Pick your monthly spend, or "I don\'t know".',
  },
  done: {
    title: (name: string) => (name ? `Almost there, ${name}` : 'Almost there'),
    saved: "We've received your answers.",
    notSaved: "We couldn't save your answers, so paste them into the booking note.",
    line: 'Last step: pick a 30-minute slot with the Nasiko team.',
    book: 'Book your assessment',
    pasteHint: 'Paste your answers into the booking note so the team can prepare.',
    copyAnswers: 'Copy your answers',
    summary: 'Your answers',
    edit: 'Edit answers',
  },
  newTab: '(opens in a new tab)',
}

export const APPLYING_AS = [
  { value: 'individual', label: 'Individual developer' },
  { value: 'team', label: 'Company or team' },
] as const

export const AGENT_COUNTS = [
  { value: '1', label: '1 agent' },
  { value: '2-5', label: '2–5 agents' },
  { value: '6-20', label: '6–20 agents' },
  { value: '21-50', label: '21–50 agents' },
  { value: '50+', label: '50+ agents' },
  { value: 'exploring', label: 'Still experimenting or planning deployment' },
] as const

export const SPEND = [
  { value: '<100', label: 'Under $100' },
  { value: '100-1k', label: '$100–$1,000' },
  { value: '1k-5k', label: '$1,000–$5,000' },
  { value: '5k-20k', label: '$5,000–$20,000' },
  { value: '20k+', label: '$20,000+' },
  { value: 'unknown', label: "I don't know / don't track it yet" },
] as const

export const CHALLENGES = [
  'High or unpredictable token costs',
  'Lack of token usage visibility',
  'Repeated tokens or oversized context',
  'Difficulty choosing the right model',
  'Model routing across providers',
  'Limited per-agent or per-workflow tracing',
  'No budgets, alerts or spending controls',
  'Slow responses or high latency',
  'Difficulty evaluating agent performance',
  'Other',
] as const
