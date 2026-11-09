/**
 * What this server keeps of a local coding agent's sessions (`GET /api/me/coding-agents/privacy`). A hosted server
 * keeps only what prices a session (tokens, cost, models, timings); a Nasiko running on the viewer's own machine may
 * also keep its chat history, if the CLI was told to sync it.
 */
import { useQuery } from '@tanstack/react-query'
import { z } from 'zod'
import { apiFetch } from '@nasiko/data-access/client'
import { isEndpointAbsent } from '@nasiko/data-access/detect'

const PRIVACY = '/api/me/coding-agents/privacy'

export const privacySchema = z.looseObject({ chat_history: z.boolean() })
export type CodingAgentPrivacy = z.infer<typeof privacySchema>

export const privacyKeys = { policy: ['harnesses', 'privacy'] as const }

/** The server's policy, or `null` from a server too old to say (no notice then: a guess could be wrong). */
export function useCodingAgentPrivacy() {
  return useQuery({
    queryKey: privacyKeys.policy,
    queryFn: async ({ signal }): Promise<CodingAgentPrivacy | null> => {
      try {
        return await apiFetch<CodingAgentPrivacy>(PRIVACY, { signal, schema: privacySchema })
      } catch (error) {
        if (isEndpointAbsent(error)) return null
        throw error
      }
    },
    // A deployment's policy: it does not change while the page is open.
    staleTime: Infinity,
    retry: false,
  })
}
