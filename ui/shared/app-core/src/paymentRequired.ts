/**
 * The last 402 Payment Required any query or mutation got (queryClient.ts): the server refused because of the
 * account's plan. The core only records it. The page that made the call still shows the server's words inline, and an
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

export function notePaymentRequired(err: unknown) {
  if (err instanceof ApiError && err.status === 402)
    store.setState({ last: { path: err.path, body: err.body, message: err.serverMessage } })
}

export const usePaymentRequired = () => useStore(store, (s) => s.last)

export const clearPaymentRequired = () => store.setState({ last: null })
