// Ported from nasiko-ui-lab@1b99bdda common/src/features/tokenops/search.ts. Local edits: none (`@/app/shell/context` and `@/lib/search` are the ported modules at the same paths); Prettier formatting.
import * as z from "zod/mini";
import { sharedSearchSchema } from "@nasiko/app-core/shell/context";
import { fallback, flag, isoDate, opt } from "@nasiko/app-core/search";

/**
 * Everything the TokenOps page shows is driven by these URL search params
 * (plan: "Filters live in the URL"). Junk values fall back to defaults instead of
 * throwing, so a hand-edited or stale link still opens the page.
 */
const SORTS = ["cost", "tokens", "operations", "latency", "hours", "name"] as const;
export type SortKey = (typeof SORTS)[number];

/** TokenOps disclosure ids (the `open` param). */
export const DISCLOSURES = ["spend", "optimise", "drivers", "perf", "month", "metrics"] as const;
export type Disclosure = (typeof DISCLOSURES)[number];

/** Normalise `open` to a canonical CSV (kept a string so the URL stays `open=spend,drivers`). */
function parseOpen(raw: string): string | undefined {
  if (raw.trim() === "all") return DISCLOSURES.join(",");
  const ids = raw.split(",").map((s) => s.trim());
  const kept = DISCLOSURES.filter((d) => ids.includes(d));
  return kept.length ? kept.join(",") : undefined;
}

/** The open disclosures as a set. */
export function openSet(open: string | undefined): Set<Disclosure> {
  return new Set(
    (open ?? "")
      .split(",")
      .filter((s): s is Disclosure => (DISCLOSURES as readonly string[]).includes(s)),
  );
}

/** `open` with one disclosure toggled (undefined when none are left). */
export function toggleOpen(open: string | undefined, id: Disclosure): string | undefined {
  const set = openSet(open);
  if (set.has(id)) set.delete(id);
  else set.add(id);
  return set.size ? DISCLOSURES.filter((d) => set.has(d)).join(",") : undefined;
}

/** The shared context (app/shell/context.ts) plus TokenOps' own keys. */
export const tokenopsSearchSchema = z.extend(sharedSearchSchema, {
  view: fallback(z.enum(["agent", "workflow"]), "agent"),
  sort: fallback(z.enum(SORTS), "cost"),
  /** Truncated, never discarded: a long query must not clear the box while typing. */
  q: opt(
    z.pipe(
      z.string(),
      z.transform((s) => s.slice(0, 200)),
    ),
  ),
  /** Selected day for the day panel (UTC date). */
  day: opt(isoDate),
  traces: opt(flag),
  more: opt(flag),
  /** Open disclosures: CSV of DISCLOSURES, or `all`. Unknown ids are dropped. */
  open: opt(z.pipe(z.string(), z.transform(parseOpen))),
});

export type TokenopsSearch = z.infer<typeof tokenopsSearchSchema>;
