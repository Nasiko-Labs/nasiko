import { z } from 'zod'
import { nameProblem } from './name'

/** URL state for the Builds page (plans/feat-deploy.md §3, §6); junk values fall back instead of throwing. */
export const BUILDS_FILTERS = ['all', 'active', 'failed', 'success'] as const
export type BuildsFilter = (typeof BUILDS_FILTERS)[number]

export const buildsSearchSchema = z.object({
  status: z.enum(BUILDS_FILTERS).optional().catch(undefined),
  /** Untrimmed while typing; the request trims. */
  q: z.string().max(200).optional().catch(undefined),
  /** Zero-based page of the non-pinned list. */
  page: z.number().int().min(0).max(1000).optional().catch(undefined),
})
export type BuildsSearch = z.infer<typeof buildsSearchSchema>

/** `/deploy`: the Build page's "Deploy again" / "Deploy as vX" prefill the name and version (design review 3). */
export const DEPLOY_METHODS = ['upload', 'github', 'registry'] as const
export type DeployMethod = (typeof DEPLOY_METHODS)[number]

export const deploySearchSchema = z.object({
  /** The method tab (design review 6); default upload. */
  method: z.enum(DEPLOY_METHODS).optional().catch(undefined),
  /** GitHub: the picked repository, `owner/name` (kept in the URL, §4.2). */
  repo: z
    .string()
    .regex(/^[\w.-]+\/[\w.-]+$/)
    .optional()
    .catch(undefined),
  // Only a name the server would accept: a link can't pre-fill shell characters into the copied CLI command.
  name: z
    .string()
    .refine((n) => nameProblem(n) === null)
    .optional()
    .catch(undefined),
  version: z.string().max(40).optional().catch(undefined),
})
export type DeploySearch = z.infer<typeof deploySearchSchema>
