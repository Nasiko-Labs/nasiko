// Ported from nasiko-ui-lab@1b99bdda common/src/features/deploy/search.ts. Local edits: `import * as z from "zod/mini"` (a named import keeps all zod locales in the bundle); `page` also accepts its
// string form (the port's search values are strings; the lab's router JSON-parsed `?page=2` to a number), still an
// integer in 0–1000 or dropped; Prettier formatting.
import * as z from "zod/mini";
import { opt } from "@nasiko/app-core/search";
import { nameProblem } from "./name";

/** URL state for the Builds page (plans/feat-deploy.md §3, §6); junk values fall back instead of throwing. */
export const BUILDS_FILTERS = ["all", "active", "failed", "success"] as const;
export type BuildsFilter = (typeof BUILDS_FILTERS)[number];

export const buildsSearchSchema = z.object({
  status: opt(z.enum(BUILDS_FILTERS)),
  /** Untrimmed while typing; the request trims. */
  q: opt(z.string().check(z.maxLength(200))),
  /** Zero-based page of the non-pinned list. */
  page: opt(
    z.pipe(
      z.union([z.number(), z.pipe(z.string(), z.transform(Number))]),
      z.int().check(z.gte(0), z.lte(1000)),
    ),
  ),
});
export type BuildsSearch = z.infer<typeof buildsSearchSchema>;

/** `/deploy`: the Build page's "Deploy again" / "Deploy as vX" prefill the name and version (design review 3). */
export const DEPLOY_METHODS = ["upload", "github", "registry"] as const;
export type DeployMethod = (typeof DEPLOY_METHODS)[number];

export const deploySearchSchema = z.object({
  /** The method tab (design review 6); default upload. */
  method: opt(z.enum(DEPLOY_METHODS)),
  /** GitHub: the picked repository, `owner/name` (kept in the URL, §4.2). */
  repo: opt(z.string().check(z.regex(/^[\w.-]+\/[\w.-]+$/))),
  // Only a name the server would accept: a link can't pre-fill shell characters into the copied CLI command.
  name: opt(z.string().check(z.refine((n) => nameProblem(n) === null))),
  version: opt(z.string().check(z.maxLength(40))),
});
export type DeploySearch = z.infer<typeof deploySearchSchema>;
