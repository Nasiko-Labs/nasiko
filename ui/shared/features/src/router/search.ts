// Ported from nasiko-ui-lab@1b99bdda common/src/features/router/search.ts. Local edits: `import * as z from "zod/mini"` (a named import keeps all zod locales in the bundle); none beyond imports and Prettier.
import * as z from "zod/mini";
import { opt } from "@nasiko/app-core/search";

/**
 * URL state for the router page (plan §4.1): the open tab (Agents when absent) and the summary tiles' filter. A junk
 * value falls back to the default instead of throwing, so a stale link still opens the page.
 */
const ROUTER_SOURCES = ["attached", "default", "none"] as const;
export type SourceFilter = (typeof ROUTER_SOURCES)[number];

export const ROUTER_TABS = ["agents", "configs", "providers"] as const;
export type RouterTab = (typeof ROUTER_TABS)[number];

export const routerSearchSchema = z.object({
  tab: opt(z.enum(["configs", "providers"])),
  source: opt(z.enum(ROUTER_SOURCES)),
});
export type RouterSearch = z.infer<typeof routerSearchSchema>;
