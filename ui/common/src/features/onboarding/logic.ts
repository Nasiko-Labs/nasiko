/** Pure onboarding rules (plan Task 1): the step model, where each persona lands, and the Overview's ticks. */
import { DRAFT_PREFIX } from '@/lib/draftKeys'
import type { Onboarding, Persona } from './types'

export const STEPS = ['welcome', 'role', 'model', 'agent', 'optimise', 'ready'] as const
export type StepId = (typeof STEPS)[number]

/** The page a persona opens first (spec §3). Data analysts get TokenOps: Weave is EE and dev only. */
export type PersonaPage =
  '/' | '/agents' | '/router' | '/tokenops' | '/harnesses' | '/chat' | '/sessions'
const OPENS: Record<Persona, PersonaPage> = {
  developer: '/agents',
  platform_engineer: '/router',
  finance: '/tokenops',
  engineering_manager: '/harnesses',
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
  role: boolean
  model: boolean
  agent: boolean
}
export function ticks(v: { persona: Persona | null; configs: number; agents: number }): Ticks {
  return { role: v.persona !== null, model: v.configs > 0, agent: v.agents > 0 }
}

/**
 * Resume at the first step not done yet; all done: Ready.
 *
 * `optimise` has no tick of its own and is skipped on resume: it configures nothing by itself, so a
 * returning user would be stopped by a page they have already read. A first run still walks through
 * it, which is the audience it exists for.
 */
export function firstOpenStep(t: Ticks): StepId {
  if (!t.role) return 'role'
  if (!t.model) return 'model'
  if (!t.agent) return 'agent'
  return 'ready'
}

/** Minutes left on step n (1-based), the prototype's estimate; none on Ready. */
export const minutesLeft = (n: number) =>
  n >= STEPS.length ? 0 : Math.max(1, 4 - Math.ceil(n / 2))

/** "Skip guide" lasts until sign-out: under the drafts prefix, so sign-out's draft clearing drops it too. */
export const skipKey = (sub: string) => `${DRAFT_PREFIX}${sub}:meta:guideSkipped`

/** The built-in providers' display names (settings/copy.ts lists the same three); anything else shows its id. */
const PROVIDER_NAMES: Record<string, string> = {
  openai: 'OpenAI',
  anthropic: 'Anthropic',
  gemini: 'Gemini',
}
export const providerName = (id: string) => PROVIDER_NAMES[id] ?? id

/** The router's built-in providers, in the order the guide offers them (router routing.ts `BUILTIN_PROVIDERS`). */
const GUIDE_PROVIDERS = ['openai', 'anthropic', 'gemini'] as const
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
