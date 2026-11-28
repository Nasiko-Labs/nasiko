/** Eval Universe reads: one flat snapshot of every task/attempt the caller has run. */
import { useQuery } from '@tanstack/react-query'
import { apiData } from '@nasiko/data-access/client'
import { universeViewSchema, type UniverseView } from './types'

export const universeKeys = {
  all: ['evals', 'universe'] as const,
}

export function useUniverse() {
  return useQuery({
    queryKey: universeKeys.all,
    queryFn: ({ signal }) =>
      apiData<UniverseView>('/api/evals/code-review/universe', {
        signal,
        schema: universeViewSchema,
      }),
  })
}
