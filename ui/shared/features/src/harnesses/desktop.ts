/**
 * The desktop app's own coding-agent onboarding (contract v3): the Nasiko desktop app serves the web app and answers
 * `/__nasiko/coding-agents/*` itself, same origin, so consent and progress never go through the server.
 *
 *   GET  /__nasiko/coding-agents/state    {desktop: true, consent, signed_in, run, backlog}
 *   POST /__nasiko/coding-agents/start    consent granted, discovery + import start   → 202 the state
 *   POST /__nasiko/coding-agents/decline  consent declined                            → 200 the state
 *   POST /__nasiko/coding-agents/retry    resend refused turns (or restart a failed run) → 202 the state
 *
 * In a browser the same path is the server's 404 or the SPA's index.html, so only a 200 JSON body that passes the
 * schema with `desktop: true` means "this is the desktop app"; anything else, a network error included, is a browser.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { z } from 'zod'
import { apiFetch } from '@nasiko/data-access/client'
import { DESKTOP_IDLE_POLL_MS, DESKTOP_STATE_POLL_MS } from './constants'
import { localRunSchema } from './runs'
import { desktopSync } from './syncStatus'

const BASE = '/__nasiko/coding-agents'

/** The desktop's queue: turns waiting and set aside, a delivery going now, when the next waiting turn is due. */
const backlogSchema = z.looseObject({
  pending: z.number(),
  rejected: z.number(),
  sending: z.boolean(),
  next_attempt_at: z.string().nullable(),
})

export const desktopStateSchema = z.looseObject({
  desktop: z.literal(true),
  consent: z.enum(['unasked', 'granted', 'declined']),
  signed_in: z.boolean(),
  run: localRunSchema.nullable(),
  // An older desktop app has no queue to report.
  backlog: backlogSchema.nullish(),
})
export type DesktopState = z.infer<typeof desktopStateSchema>
export type DesktopAction = 'start' | 'decline' | 'retry'

export const desktopKeys = { state: ['harnesses', 'onboarding', 'desktop'] as const }

/** The desktop app's state, or null in a browser. Every failure (404, the SPA's HTML, a body that isn't the desktop's,
 *  a network error) means "not the desktop"; only a cancelled read is rethrown. */
async function probeDesktop(signal: AbortSignal): Promise<DesktopState | null> {
  try {
    const parsed = desktopStateSchema.safeParse(
      await apiFetch<unknown>(`${BASE}/state`, { signal }),
    )
    return parsed.success ? parsed.data : null
  } catch (err) {
    if (signal.aborted) throw err
    return null
  }
}

/** How often to re-read the desktop's state: every `DESKTOP_STATE_POLL_MS` while it syncs, every
 *  `DESKTOP_IDLE_POLL_MS` otherwise (hooks queue turns at any time; a local call, no network); never in a browser. */
export function desktopPollInterval(state: DesktopState | null | undefined): number | false {
  if (!state) return false
  return desktopSync(state).syncing ? DESKTOP_STATE_POLL_MS : DESKTOP_IDLE_POLL_MS
}

/** The desktop probe (one query for Monitor's import state and the Harnesses card), re-read by `desktopPollInterval`. */
export function useDesktopState(enabled = true) {
  return useQuery({
    queryKey: desktopKeys.state,
    queryFn: ({ signal }) => probeDesktop(signal),
    enabled,
    // A probe that never fails (a browser answers null), so there is nothing to retry.
    retry: false,
    refetchOnWindowFocus: false,
    staleTime: Infinity,
    refetchInterval: (q) => desktopPollInterval(q.state.data),
  })
}

/** One of the desktop's POSTs. Its answer is the new state, shown at once; a mutation, never retried. */
export function useDesktopAction() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: async (action: DesktopAction) =>
      desktopStateSchema.parse(
        await apiFetch<DesktopState>(`${BASE}/${action}`, {
          method: 'POST',
          schema: desktopStateSchema,
        }),
      ),
    onSuccess: (state) => qc.setQueryData(desktopKeys.state, state),
  })
}
