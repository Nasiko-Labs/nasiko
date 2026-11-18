/** Pure onboarding rules (plan Task 1): the step model, where each persona lands, and the home's Setup guide ticks. */
import type { Onboarding, Persona } from './types'

/** `start` is one step for the first agent and the model key (every model call is billed to the user's own key, so Continue
 *  waits for one); `harness` ends the guide with the CLI and coding harnesses (user request 2026-11-05: no Spend less, no Ready).
 *  No role step (user request 2026-11-09): the guide no longer asks for a persona. */
export const STEPS = ['welcome', 'start', 'harness'] as const
export type StepId = (typeof STEPS)[number]

/** The page a persona opens first (spec §3). Data analysts get TokenOps: Weave is EE and dev only. Nobody is sent to
 *  Coding harnesses: connecting a local harness comes after a first chat, not during setup. */
export type PersonaPage = '/' | '/agents' | '/router' | '/tokenops' | '/chat' | '/sessions'
const OPENS: Record<Persona, PersonaPage> = {
  developer: '/agents',
  platform_engineer: '/router',
  finance: '/tokenops',
  engineering_manager: '/tokenops',
  product_manager: '/chat',
  data_analyst: '/tokenops',
  support_lead: '/chat',
  sre: '/sessions',
  leadership: '/',
}
export const opensFor = (p: Persona): PersonaPage => OPENS[p]

/** The guide takes over `/` only for a known first-time user who hasn't skipped it this session. No answer (absent
 *  route, error, still loading) is never "first time": the app must not block on this read. */
export function guideDue(data: Onboarding | undefined, skipped: boolean): boolean {
  return !!data?.is_first_time_user && !skipped
}

export interface Ticks {
  model: boolean
  agent: boolean
}
export function ticks(v: { configs: number; agents: number }): Ticks {
  return { model: v.configs > 0, agent: v.agents > 0 }
}

/** Resume at the first step not done yet; all done: the coding harnesses step, which has no tick of its own. */
export function firstOpenStep(t: Ticks): StepId {
  if (!t.model || !t.agent) return 'start'
  return 'harness'
}

/** Minutes left on step n (1-based) of the two-minute guide: two on Welcome, one on the Agent and key step, none on the
 *  last (user review 2026-11-05). */
export const minutesLeft = (n: number) =>
  n >= STEPS.length ? 0 : Math.ceil((STEPS.length - n + 1) / 2)

/** How long the Agent and key step's Continue stays off, counting down in its label, so the step is read before it is
 *  passed (user request 2026-11-05: that step only). */
export const CONTINUE_HOLD_SECONDS = 5

/** A prebuilt agent the Agent and key step offers to deploy: a Nasiko agent package on the public registry. */
export interface SeededAgent {
  /** The agent's name once deployed: the repository's last segment, as the server's registry import names it. */
  name: string
  /** What the guide shows. */
  title: string
  line: string
  /** What the registry import is sent (`host/owner/name:tag`). */
  reference: string
  runs: number
  /** Share of `runs` that succeeded, 0–1. */
  success: number
}

const seeded = (
  name: string,
  title: string,
  line: string,
  runs: number,
  success: number,
): SeededAgent => ({
  name,
  title,
  line,
  reference: `registry.nasiko.com/nasiko/${name}:1.0.0`,
  runs,
  success,
})

/**
 * The prebuilt agents (user request 2026-11-09), Nasiko's packages on registry.nasiko.com. ponytail: the run and
 * success figures are demo values; a server list of seeded agents with their real metrics replaces them.
 */
export const SEEDED_AGENTS: readonly SeededAgent[] = [
  seeded(
    'meeting-notes-processor',
    'Meeting Notes Processor',
    'Turns a transcript into decisions and actions.',
    5_200,
    0.98,
  ),
  seeded('summarizer', 'Summarizer', 'Condenses long threads, calls and reports.', 3_400, 0.9),
  seeded('reply-drafter', 'Reply Drafter', 'Drafts replies to emails and messages.', 2_900, 0.93),
  seeded(
    'task-breakdown',
    'Task Breakdown',
    'Splits a goal into ordered, doable tasks.',
    1_800,
    0.95,
  ),
  seeded('humanizer', 'Humanizer', 'Rewrites stiff text so it reads naturally.', 2_100, 0.92),
  seeded('translator', 'Translator', 'Translates text while keeping its tone.', 2_600, 0.96),
]

/** The agent Home's try-a-run card runs (overview/tryRun.ts): the first of the agents every new workspace is given. */
export const MANDATORY = 'meeting-notes-processor'

/** The guide opens on its own once per user (user request 2026-11-05): set when it does, and not under the drafts prefix,
 *  so sign-out keeps it and an unfinished guide never comes back by itself. The Setup guide still reopens it. */
export const skipKey = (sub: string) => `openruntime.guideShown:${sub}`

/** The built-in providers' display names (settings/copy.ts lists the same three); anything else shows its id. */
const PROVIDER_NAMES: Record<string, string> = {
  openai: 'OpenAI',
  anthropic: 'Anthropic',
  gemini: 'Gemini',
  openrouter: 'OpenRouter',
}
export const providerName = (id: string) => PROVIDER_NAMES[id] ?? id

/** The router's built-in providers, in the order the guide offers them (router routing.ts `BUILTIN_PROVIDERS`). */
const GUIDE_PROVIDERS = ['openai', 'anthropic', 'gemini', 'openrouter'] as const
/** The pricing catalog files Gemini's models under `google`; the router's provider is `gemini`. */
const CATALOG_ALIASES: Record<string, string> = { google: 'gemini' }

/** The built-in providers the catalog has models for, each with its model names (catalog order). */
export function guideProviders(
  catalog: readonly { provider: string; models: readonly { model: string }[] }[] | undefined,
): { provider: string; models: string[] }[] {
  const by = new Map<string, string[]>()
  for (const p of catalog ?? []) {
    const id = CATALOG_ALIASES[p.provider] ?? p.provider
    by.set(id, [...(by.get(id) ?? []), ...p.models.map((m) => m.model)])
  }
  return GUIDE_PROVIDERS.filter((id) => by.get(id)?.length).map((id) => ({
    provider: id,
    models: by.get(id) ?? [],
  }))
}

/**
 * The home's default scope for a persona (plans/feat-agent-centric.md PR2): people who mostly build or use their own
 * agents start on Yours, people who run or pay for the fleet on Fleet. No persona (unset, skipped or no onboarding read):
 * Yours for members, Fleet for superusers.
 */
const FLEET_PERSONAS: readonly Persona[] = [
  'platform_engineer',
  'sre',
  'engineering_manager',
  'leadership',
  'finance',
]
export const homeScopeFor = (persona: Persona | null, isSuperuser: boolean): 'yours' | 'fleet' =>
  persona ? (FLEET_PERSONAS.includes(persona) ? 'fleet' : 'yours') : isSuperuser ? 'fleet' : 'yours'
