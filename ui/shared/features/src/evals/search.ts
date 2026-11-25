import { z } from 'zod/mini'
import { opt } from '@nasiko/ui/lib/search'

// Here, not in logic.ts: the route's search schema loads with the shell, and logic.ts would come with it.
/** `/evals`'s URL state: the dataset being built or picked from, and the run being compared, so either can be
 * reopened later. Junk values fall back, so a stale link still opens the sessions. */
export const evalsSearchSchema = z.object({
  dataset: opt(z.string().check(z.maxLength(64))),
  run: opt(z.string().check(z.maxLength(64))),
})
export type EvalsSearch = z.infer<typeof evalsSearchSchema>
