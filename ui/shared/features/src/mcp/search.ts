// Ported from nasiko-ui-lab@1b99bdda common/src/features/mcp/search.ts. Local edits: `import * as z from "zod/mini"` (a named import keeps all zod locales in the bundle); none beyond imports and
// Prettier formatting.
import * as z from "zod/mini";
import { opt } from "@nasiko/app-core/search";

// Here, not in logic.ts: the route's search schema loads with the shell, and logic.ts would come with it.
/** `?view=`: the legacy module nav's scopes. Ownership scopes are custom servers only. */
export const VIEWS = ["yours", "shared", "toolkits"] as const;
export type View = (typeof VIEWS)[number];
export const TABS = ["all", "available", "connected"] as const;
export type Tab = (typeof TABS)[number];

/** URL state for the MCP pages (plans/feat-mcp.md §2, §5). Junk values fall back, so a stale link still opens. */
export const catalogSearchSchema = z.object({
  /** Untrimmed while typing; matching trims. */
  q: opt(z.string().check(z.maxLength(200))),
  view: opt(z.enum(VIEWS)),
  tab: opt(z.enum(TABS)),
});
export type CatalogSearch = z.infer<typeof catalogSearchSchema>;

export type DetailTab = "overview" | "agents" | "access" | "logs" | "settings";
/** Unknown tabs stay strings so the page can fall back to Overview with replace (as Agents does). */
export const detailSearchSchema = z.object({
  tab: opt(z.string().check(z.trim(), z.maxLength(40))),
  /** The agent whose access the Agents tab shows. */
  agent: opt(z.uuid()),
});
export type DetailSearch = z.infer<typeof detailSearchSchema>;
