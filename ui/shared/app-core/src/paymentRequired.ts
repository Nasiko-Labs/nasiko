/**
 * The last refusal any query or mutation got that paying or a plan change could resolve (queryClient.ts): a 402 Payment
 * Required, or a 403 or 409 whose body lists next steps (`actions`). The core only records it. The page that made the call still shows the server's words inline, and an
 * edition that sells plans says what to do about it through its `shellNotice` slot. It ends with the session
 * (queryClient.ts clears it with the cache).
 */
import { useStore } from 'zustand'
import { createStore } from 'zustand/vanilla'
import { ApiError } from '@nasiko/data-access/client'

export interface PaymentRequired {
  path: string
  body: unknown
  /** The server's own words (`ApiError.serverMessage`). */
  message: string | null
}

const store = createStore<{ last: PaymentRequired | null }>(() => ({ last: null }))

/** A 403 (a quota) or 409 (a state conflict) is a plan refusal only when the server attached next steps to it. */
const hasActions = (body: unknown) =>
  !!body && typeof body === 'object' && Array.isArray((body as { actions?: unknown }).actions)

export function notePaymentRequired(err: unknown) {
  if (!(err instanceof ApiError)) return
  if (err.status === 402 || ((err.status === 403 || err.status === 409) && hasActions(err.body)))
    store.setState({ last: { path: err.path, body: err.body, message: err.serverMessage } })
}

export const usePaymentRequired = () => useStore(store, (s) => s.last)

export const clearPaymentRequired = () => store.setState({ last: null })
