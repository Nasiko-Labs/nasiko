import { z } from 'zod/mini'
import { flag, opt } from '@nasiko/ui/lib/search'
import { nameProblem } from './name'

/** URL state for the Builds page (plans/feat-deploy.md §3, §6); junk values fall back instead of throwing. */
export const BUILDS_FILTERS = ['all', 'active', 'failed', 'success'] as const
export type BuildsFilter = (typeof BUILDS_FILTERS)[number]

export const buildsSearchSchema = z.object({
  status: opt(z.enum(BUILDS_FILTERS)),
  /** Untrimmed while typing; the request trims. */
  q: opt(z.string().check(z.maxLength(200))),
  /** Zero-based page of the non-pinned list. */
  page: opt(z.int().check(z.gte(0), z.lte(1000))),
})
export type BuildsSearch = z.infer<typeof buildsSearchSchema>

/** `/deploy`: the Build page's "Deploy again" / "Deploy as vX" prefill the name and version (design review 3). */
export const DEPLOY_METHODS = ['upload', 'github', 'registry'] as const
export type DeployMethod = (typeof DEPLOY_METHODS)[number]

export const deploySearchSchema = z.object({
  /** The method tab (design review 6); default upload. */
  method: opt(z.enum(DEPLOY_METHODS)),
  /** GitHub: the picked repository, `owner/name` (kept in the URL, §4.2). */
  repo: opt(z.string().check(z.regex(/^[\w.-]+\/[\w.-]+$/))),
  // Only a name the server would accept: a link can't pre-fill shell characters into the copied CLI command.
  name: opt(z.string().check(z.refine((n) => nameProblem(n) === null))),
  version: opt(z.string().check(z.maxLength(40))),
  /**
   * A new version of the agent `name` names: the name can't change. The server keys an agent on owner and name
   * (`agents/upload.rs`, `ON CONFLICT (owner_id, name)`), so another name would deploy a second agent, not a version.
   */
  newVersion: opt(flag),
})

/** The name a new-version deploy is fixed to, or null when the name is free. */
export const lockedName = (search: DeploySearch): string | null =>
  search.newVersion && search.name ? search.name : null
export type DeploySearch = z.infer<typeof deploySearchSchema>
