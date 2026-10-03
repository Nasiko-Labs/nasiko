/**
 * Router errors (plan §4.9): each entry is problem, cause and action, matched by status plus the server's
 * plain-text message (nasiko-server @ cb3aaf0c returns text, not codes). Anything unmapped is shown verbatim
 * with its status. Keys and secret values never appear in the text we show.
 */
import { ApiError } from '@/lib/api/client'
import { copy } from './copy'

export interface ErrorView {
  problem: string
  cause?: string
  action?: string
  /** The field the error belongs to, when the form can show it there. */
  field?: 'name' | 'secret' | 'model' | 'pin' | 'provider' | 'baseUrl' | 'limit' | 'thresholds'
}

type Rule = {
  status: number | ((s: number) => boolean)
  match?: RegExp
  view: (msg: string) => ErrorView
}

// R-L1..R-L10: these match the server's text; error codes would make them robust.
const RULES: Rule[] = [
  {
    status: 504,
    match: /classifier preview timed out/,
    view: () => ({ problem: copy.classifierTimedOut, action: copy.classifierTimedOutFix }),
  },
  {
    status: 429,
    view: () => ({ problem: copy.classifierRateLimited, action: copy.classifierRateLimitedFix }),
  },
  { status: 409, match: /budget changed elsewhere/, view: () => ({ problem: copy.budgetChanged }) },
  {
    status: 409,
    match: /budget already exists/,
    view: () => ({
      problem: 'This scope already has a budget.',
      action: 'Edit that budget instead.',
    }),
  },
  {
    status: 400,
    match: /limit_usd/,
    view: () => ({ problem: copy.limitPositive, field: 'limit' }),
  },
  {
    status: 400,
    match: /thresholds must/,
    view: () => ({ problem: copy.thresholdRange, field: 'thresholds' }),
  },
  {
    status: 409,
    match: /already exists/,
    view: () => ({
      problem: 'That name is taken.',
      cause: 'Another of your configs has this name.',
      action: 'Pick another name.',
      field: 'name',
    }),
  },
  {
    status: 409,
    match: /attached to/,
    view: (m) => ({
      problem: 'This config is in use.',
      cause: m,
      action: 'Change the agents’ routing first, then delete it.',
    }),
  },
  {
    status: 409,
    match: /still referenced/,
    view: () => ({
      problem: 'This provider is still used by configs.',
      action: 'Repoint those configs first.',
    }),
  },
  {
    status: 400,
    match: /not owned by the agent owner/,
    view: () => ({
      problem: 'That config can’t be attached to this agent.',
      cause: 'An attached config must belong to the agent’s owner.',
      action: 'Pick one of the owner’s configs.',
    }),
  },
  {
    status: 400,
    match: /pinned_model must not be empty/,
    view: () => ({
      problem: 'Choose the model to pin.',
      cause: 'The pin model was empty.',
      field: 'pin',
    }),
  },
  {
    status: 400,
    match: /secret '.*' not found/,
    view: () => ({
      problem: 'That saved key no longer exists.',
      cause: 'It was deleted after this page loaded.',
      action: 'Add the key again, or pick another saved key.',
      field: 'secret',
    }),
  },
  {
    status: 400,
    match: /secret name/,
    view: (m) => ({
      problem: 'That secret name isn’t allowed.',
      cause: m,
      action: 'Use uppercase letters, digits and _.',
      field: 'secret',
    }),
  },
  {
    status: 400,
    match: /unsupported provider|not a registered custom provider/,
    view: (m) => ({
      problem: 'The router can’t use that provider.',
      cause: m,
      action: 'Pick a built-in provider or a registered custom one.',
      field: 'provider',
    }),
  },
  {
    status: 400,
    match: /model/,
    view: (m) => ({
      problem: 'The model settings aren’t valid.',
      cause: m,
      action: 'Set a model, or at least one tier model.',
      field: 'model',
    }),
  },
  {
    status: 400,
    match: /base_url|display_name|api_key is required/,
    view: (m) => ({ problem: 'The provider details aren’t complete.', cause: m, field: 'baseUrl' }),
  },
  { status: 403, view: () => ({ problem: 'Only the owner or a superuser can do this.' }) },
  {
    status: 404,
    view: () => ({
      problem: 'This was deleted elsewhere.',
      action: 'The page reloaded the latest state.',
    }),
  },
  // S-6: a failed secret write returns database text; never show it.
  {
    status: 500,
    match: /secret/,
    view: () => ({
      problem: 'Couldn’t save the key.',
      cause: 'OpenRuntime couldn’t store it.',
      action: 'Try again.',
      field: 'secret',
    }),
  },
  {
    status: (s) => s === 502 || s === 504,
    view: () => ({ problem: copy.errDown, action: copy.errDownFix }),
  },
]

/** A config delete's 409 "config is attached to N agent(s)": N, or null for any other error. */
export function inUseCount(err: unknown): number | null {
  if (!(err instanceof ApiError) || err.status !== 409) return null
  const m = /attached to (\d+)/.exec(err.serverMessage ?? '')
  return m ? Number(m[1]) : null
}

/** The view for any error the router's queries or mutations throw. */
export function routerError(err: unknown): ErrorView {
  if (!(err instanceof ApiError))
    return {
      problem: 'Something went wrong.',
      cause: err instanceof Error ? err.message : undefined,
    }
  const msg = err.serverMessage ?? ''
  for (const r of RULES) {
    const statusOk = typeof r.status === 'number' ? r.status === err.status : r.status(err.status)
    if (statusOk && (!r.match || r.match.test(msg))) return r.view(msg)
  }
  return { problem: copy.errUnmapped(msg || 'no message', err.status), action: 'Try again.' }
}

/**
 * A save that carried a key failed after the key may have been written (eng #6, R-L14). llm_configs.rs `create` and
 * `update` return every 400, 404 and 409 before `ensure_secret` runs, so only a 5xx or a lost response can follow it.
 */
export function keySaveFailed(err: unknown): boolean {
  return !(err instanceof ApiError) || err.status >= 500
}
