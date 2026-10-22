// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/search.ts. Local edits: Prettier formatting.
/**
 * The home's search params (plans/feat-agent-centric.md PR2): the window (`?range=`, the Overview's own key, so links to
 * `/?range=30d` keep working) and the scope. Both optional: the landing URL stays a bare `/`. Loaded with the shell, so
 * zod/mini only.
 *
 * Scope: `yours`, `fleet`, or `unit:<id>`, an org level an edition layer adds (PR2c, the `homeOrgScope` slot). The core
 * only checks its shape; what a unit holds is the layer's.
 *
 * Health (PR2d): the chart band's health bar narrows the table to one rating.
 */
import * as z from "zod/mini";
import { opt } from "@nasiko/app-core/search";
import { RANGES } from "../overview/search";

export type UnitScope = `unit:${string}`;
export type Scope = "yours" | "fleet" | UnitScope;

/** A unit id is a UUID (org_units.rs `Path<Uuid>`): nothing else reaches a read. */
const UNIT = /^unit:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export const isUnitScope = (s: Scope | null | undefined): s is UnitScope =>
  !!s && s.startsWith("unit:");
/** The unit id in `unit:<id>`. */
export const unitOf = (s: UnitScope) => s.slice("unit:".length);

const scopeSchema = z.union([
  z.enum(["yours", "fleet"]),
  z.pipe(
    z.string().check(z.regex(UNIT)),
    z.transform((s) => s as UnitScope),
  ),
]);

export const homeSearchSchema = z.object({
  range: opt(z.enum(RANGES)),
  /** No static default: the persona decides when the URL doesn't (`scope.ts`). */
  scope: opt(scopeSchema),
  /** The table's health filter (PR2d, decision #62): the Agents catalog's key and values; Not rated is `unknown`. */
  health: opt(z.enum(["action", "watch", "healthy", "unknown"])),
});
export type HomeSearch = z.infer<typeof homeSearchSchema>;
